//! Global hotkey runtime surface for the SwiftUI redesign.
//!
//! This does not reimplement hotkeys in Swift. It starts the same macOS
//! `CGEventTap` listener used by the legacy daemon and dispatches emitted
//! `HotkeyEvent`s into the existing `RecordingController` state machine.

use std::future::Future;
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::sync::{Arc, Mutex, OnceLock, RwLock};
use std::time::Duration;

use codescribe::controller::{HotkeyAction, HotkeyInput, HotkeyType, RecordingController, State};
use codescribe::os::hold_badge::BadgeMode;
use codescribe::os::hotkeys::{self, HoldAction, HoldMode, HotkeyEvent};
use codescribe::os::permissions::{PermissionStatus, check_accessibility, check_input_monitoring};
use codescribe::os::shortcut_registry::{detect_hotkey_conflicts, fn_tap_intercept_note};
use codescribe::os::tray_status::{self, TrayStatus};
use codescribe::os::{clipboard, notifications};
use codescribe_core::config::{
    Config, FormattingPolicy, ModeBinding, ShortcutBinding, UserSettings, WorkMode,
};
use codescribe_core::ipc::{EngineEventWire, IpcEventPayload};
use crossbeam_channel::unbounded;
use tokio::runtime::Handle;
use tokio::sync::broadcast::error::RecvError;

use crate::agent_delivery::{
    CsAgentDeliveryListener, set_delivery_listener, spawn_delivery_forwarder,
};
use crate::recording::{
    CsAnnotationKind, CsLayerSummary, CsTranscriptProjectionEvent, CsTranscription,
    CsTranscriptionListener,
};
use crate::{CsError, CsLanguage, application_runtime};

/// Shared process-wide slot for the lazily-created `RecordingController`.
/// Mutex so the first hotkey/FFI path wins construction; `Option` until first use.
type SharedController = Arc<Mutex<Option<Arc<RecordingController>>>>;
/// Shared process-wide slot for the Swift overlay transcription listener.
/// `RwLock` because the forwarder reads on every event and Swift writes once.
type SharedListener = Arc<RwLock<Option<Arc<dyn CsTranscriptionListener>>>>;
/// Shared process-wide slot for the Swift UI-only app-action listener.
/// Same shape as the overlay listener; never carries audio or model payload.
type SharedAppActionListener = Arc<RwLock<Option<Arc<dyn CsAppActionListener>>>>;

/// Foreign callback for UI-only global commands. These actions are deliberately
/// separate from `CsTranscriptionListener`: they carry no audio or model payload
/// and must never enter the recording controller path.
#[uniffi::export(with_foreign)]
pub trait CsAppActionListener: Send + Sync {
    /// Bring the Agent surface forward. UI-only — must not touch the mic.
    fn on_show_agent(&self);
}

/// Capture ownership sentinel: no lane currently owns the microphone.
const CAPTURE_OWNER_NONE: u8 = 0;
/// Capture ownership: the one shared `RecordingController` owns the mic.
const CAPTURE_OWNER_CONTROLLER: u8 = 1;
/// Process-wide start gate. Every Dictation/Agent/Assistive gesture enters the
/// same controller, so this protects one capture rather than mediating lanes.
static CAPTURE_OWNER: AtomicU8 = AtomicU8::new(CAPTURE_OWNER_NONE);
/// A quit request must not wait forever for provider/network work hidden in the
/// recording stop path. The controller still owns best-effort cleanup after
/// this bounded application-level wait expires.
const RECORDING_SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(10);

/// Whether this event would begin a NEW controller capture session, and therefore
/// has to claim capture ownership first. Deliberately narrow: only the two
/// toggles and a hold key-down start a session; every other event either
/// continues or ends one that already owns the mic.
fn event_can_start_capture(event: &HotkeyEvent) -> bool {
    matches!(
        event,
        HotkeyEvent::ToggleNormal
            | HotkeyEvent::ToggleRaw
            | HotkeyEvent::ToggleAssistive
            | HotkeyEvent::Hold {
                action: HoldAction::Down,
                mode: HoldMode::Raw | HoldMode::Chat | HoldMode::Selection,
            }
    )
}

/// Agent/Assistive recording still fronts the Agent surface, but the UI callback
/// is notification only; audio and transcript events continue to the controller.
///
/// Mid-hold attach (`AttachSelection`) and leftover `HoldUpdate` Chat must not
/// front Agent — they would hide the overlay and look like the take died.
fn event_targets_agent_ui(event: &HotkeyEvent) -> bool {
    matches!(
        event,
        HotkeyEvent::ToggleAssistive
            | HotkeyEvent::Hold {
                action: HoldAction::Down,
                mode: HoldMode::Chat | HoldMode::Selection,
            }
    )
}

/// Process-global slot for the lazily-created `RecordingController`.
fn shared_controller() -> SharedController {
    /// Once-initialized shared controller store for this process.
    static CONTROLLER: OnceLock<SharedController> = OnceLock::new();
    Arc::clone(CONTROLLER.get_or_init(|| Arc::new(Mutex::new(None))))
}

/// Process-global slot for the Swift overlay listener. `RwLock` because the
/// event forwarder reads it on every broadcast and Swift writes it once.
fn shared_listener() -> SharedListener {
    /// Once-initialized overlay transcription-listener store.
    static LISTENER: OnceLock<SharedListener> = OnceLock::new();
    Arc::clone(LISTENER.get_or_init(|| Arc::new(RwLock::new(None))))
}

/// Process-global slot for the Swift app-action listener (UI-only commands).
fn shared_app_action_listener() -> SharedAppActionListener {
    /// Once-initialized app-action listener store (UI-only commands).
    static LISTENER: OnceLock<SharedAppActionListener> = OnceLock::new();
    Arc::clone(LISTENER.get_or_init(|| Arc::new(RwLock::new(None))))
}

/// Snapshot the registered app-action listener, if Swift has installed one.
/// Cloned out of the lock so the caller never holds it across a foreign call.
fn current_app_action_listener() -> Option<Arc<dyn CsAppActionListener>> {
    shared_app_action_listener()
        .read()
        .unwrap_or_else(|e| e.into_inner())
        .as_ref()
        .map(Arc::clone)
}

/// Single routing decision for every hotkey event, kept pure over injected
/// callbacks so the whole contract is unit-testable without a live tap,
/// controller or runtime.
///
/// Agent/Assistive events notify the Agent window and then continue through the
/// same recording callback as Dictation. Only `ShowAgent` and `InsertHere` are
/// UI-only commands.
fn route_hotkey_event<F, G>(
    event: HotkeyEvent,
    app_action_listener: Option<Arc<dyn CsAppActionListener>>,
    dispatch_recording: F,
    dispatch_deferred_insert: G,
) where
    F: FnOnce(HotkeyEvent),
    G: FnOnce(),
{
    if event_targets_agent_ui(&event) {
        tracing::info!(
            ?event,
            "Assistive command: dispatching shared controller capture"
        );
        if let Some(listener) = app_action_listener.as_ref() {
            listener.on_show_agent();
        }
    }
    match event {
        HotkeyEvent::ShowAgent => {
            tracing::info!("Agent summon command: dispatching UI-only app action");
            if let Some(listener) = app_action_listener {
                listener.on_show_agent();
            }
        }
        HotkeyEvent::InsertHere => dispatch_deferred_insert(),
        recording_event => dispatch_recording(recording_event),
    }
}

/// Deliver the armed "Paste Here" transcript and always tell the user what
/// happened — a silent no-op on an explicit user gesture reads as a broken
/// hotkey, so nothing-to-insert and expiry both surface a notification.
fn deliver_deferred_insert_and_notify() {
    match clipboard::deliver_deferred_insert() {
        Ok(clipboard::DeferredInsertDelivery::Delivered) => {
            tracing::info!("Deferred transcript delivered and clipboard restore scheduled");
        }
        Ok(
            clipboard::DeferredInsertDelivery::NothingToInsert
            | clipboard::DeferredInsertDelivery::Expired,
        ) => notifications::notify("Codescribe", "Nothing to insert"),
        Err(error) => {
            tracing::warn!(%error, "Deferred transcript delivery failed");
            notifications::notify("Codescribe", "Couldn't insert the armed transcript");
        }
    }
}

/// Get the shared controller, creating it (and its event forwarder) on first
/// use. Lazy by design: `start()` installs the tap without paying
/// `Config::load()` until the user actually invokes a shortcut.
fn ensure_controller(
    controller_store: &SharedController,
    handle: Handle,
) -> Arc<RecordingController> {
    let mut guard = controller_store.lock().unwrap_or_else(|e| e.into_inner());
    Arc::clone(guard.get_or_insert_with(|| {
        let controller = Arc::new(RecordingController::new_without_keychain());
        spawn_event_forwarder(Arc::clone(&controller), handle);
        controller
    }))
}

/// Snapshot the shared controller WITHOUT creating one. Query surfaces use this
/// so a mere status read never triggers controller construction.
fn current_controller(controller_store: &SharedController) -> Option<Arc<RecordingController>> {
    controller_store
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .as_ref()
        .map(Arc::clone)
}

fn release_capture_ownership_for_shutdown() {
    CAPTURE_OWNER.store(CAPTURE_OWNER_NONE, Ordering::SeqCst);
}

async fn await_recording_shutdown<F>(future: F, timeout: Duration) -> Result<(), CsError>
where
    F: Future<Output = Result<(), CsError>>,
{
    tokio::time::timeout(timeout, future)
        .await
        .map_err(|_| CsError::Recording {
            msg: format!(
                "application shutdown timed out after {:.1}s while stopping recording",
                timeout.as_secs_f64()
            ),
        })?
}

/// Stop accepting gestures, finish an active microphone take on the app-owned
/// runtime, and drop the process-global controller slot before worker teardown.
pub(crate) fn shutdown_application_controller() -> Result<(), CsError> {
    hotkeys::shutdown_global_hotkey_manager();
    let controller = shared_controller()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .take();
    let stop_result = match controller {
        Some(controller) => application_runtime::block_on(await_recording_shutdown(
            async move {
                if matches!(
                    controller.current_state().await,
                    State::RecHold | State::RecToggle | State::Conversation
                ) {
                    controller
                        .stop_recording_from_external_surface()
                        .await
                        .map_err(|error| CsError::Recording {
                            msg: format!("application shutdown could not stop recording: {error}"),
                        })?;
                }
                Ok(())
            },
            RECORDING_SHUTDOWN_TIMEOUT,
        ))
        .and_then(|result| result),
        None => Ok(()),
    };
    release_capture_ownership_for_shutdown();
    stop_result
}

/// Collapse a latched paste-target app name to `None` when it carries no
/// information, so Swift never renders a blank or whitespace-only app label as
/// if it were a known target.
fn normalize_paste_target_app_name(name: Option<String>) -> Option<String> {
    name.and_then(|value| {
        let trimmed = value.trim();
        (!trimmed.is_empty()).then(|| trimmed.to_string())
    })
}

/// Process-global tokio runtime handle, captured when the hotkey listener
/// starts. Lets sync surfaces (e.g. the Settings config writer) schedule async
/// controller work on the runtime the shared controller already lives on.
fn shared_runtime_handle() -> &'static OnceLock<Handle> {
    /// Once-initialized tokio handle captured when the hotkey tap starts.
    static HANDLE: OnceLock<Handle> = OnceLock::new();
    &HANDLE
}

/// Push freshly-persisted settings into the live shared controller so a Settings
/// write takes effect without an app restart (language, AI formatting, hold
/// delays, …). No-op before the runtime/controller exist — a controller created
/// later already loads fresh config on construction. Runs `set_config` on the
/// hotkey runtime the controller lives on, mirroring how `start()` drives it.
pub(crate) fn refresh_live_controller_config() {
    let Some(handle) = shared_runtime_handle().get() else {
        return;
    };
    let Some(controller) = current_controller(&shared_controller()) else {
        return;
    };
    handle.spawn(async move {
        controller.set_config(Config::load_without_keychain()).await;
    });
}

/// Pump the controller's broadcast stream into the registered Swift listener for
/// the controller's lifetime, and release controller capture ownership on every
/// return to `idle`.
///
/// The listener is resolved per event rather than captured, so a listener that
/// registers after the forwarder starts still receives everything from that
/// point on. Lag is survivable (keep forwarding); only a closed channel ends the
/// task.
fn spawn_event_forwarder(controller: Arc<RecordingController>, handle: Handle) {
    let listener_store = shared_listener();
    let mut events = controller.subscribe_events();
    handle.spawn(async move {
        loop {
            let event = match events.recv().await {
                Ok(event) => event,
                // Lagged: the broadcast channel (cap 256) overflowed during a
                // burst of dictation events and dropped `skipped` messages. That
                // is recoverable — keep forwarding subsequent events instead of
                // tearing the listener bridge down permanently.
                Err(RecvError::Lagged(skipped)) => {
                    eprintln!(
                        "Hotkey event forwarder lagged; dropped {skipped} broadcast event(s)"
                    );
                    continue;
                }
                // Closed: the controller (sender) was dropped — nothing more will
                // ever arrive, so end the forwarder task.
                Err(RecvError::Closed) => break,
            };
            if matches!(
                &event.payload,
                IpcEventPayload::StateChange { to, .. } if to == "idle"
            ) {
                let _ = CAPTURE_OWNER.compare_exchange(
                    CAPTURE_OWNER_CONTROLLER,
                    CAPTURE_OWNER_NONE,
                    Ordering::AcqRel,
                    Ordering::Acquire,
                );
            }
            let listener = listener_store
                .read()
                .unwrap_or_else(|e| e.into_inner())
                .as_ref()
                .map(Arc::clone);
            let Some(listener) = listener else {
                continue;
            };
            forward_event_to_listener(event.payload, listener);
        }
    });
}

/// Translate one IPC payload into the Swift listener's callback vocabulary and
/// keep the tray status in step.
///
/// Stateless router by design — every idempotency concern lives on the Swift
/// side. `Drop` and `Stats` are intentionally not forwarded: they are telemetry,
/// not user-visible transcript truth.
fn forward_event_to_listener(payload: IpcEventPayload, listener: Arc<dyn CsTranscriptionListener>) {
    match payload {
        IpcEventPayload::StateChange { to, .. } => match to.as_str() {
            "rec_hold" | "rec_toggle" | "conversation" => {
                // A real state transition resolves any pending optimistic
                // "preparing" overlay, so the post-dispatch compensator must not
                // also fire a terminal stop for it.
                PREPARING_PENDING.store(false, Ordering::Release);
                tray_status::update_tray_status(TrayStatus::Listening);
                listener.on_recording_started();
            }
            "busy" => {
                // Capture ended; the controller is running the final transcription
                // pass. Surface it as a distinct "finalising" beat BEFORE the
                // terminal `idle`→stopped, so the native hold-release / toggle stop
                // can show a "transcribing" phase instead of the still-pulsing
                // live-capture UI. Does not touch PREPARING_PENDING — a real Rec
                // state (rec_hold/rec_toggle) always precedes Busy and already
                // cleared it.
                tray_status::update_tray_status(TrayStatus::Thinking);
                listener.on_recording_finalising();
            }
            "idle" => {
                PREPARING_PENDING.store(false, Ordering::Release);
                tray_status::update_tray_status(TrayStatus::Idle);
                listener.on_recording_stopped();
            }
            _ => {}
        },
        IpcEventPayload::FinalTranscript { text } => listener.on_final_transcript_ready(text),
        IpcEventPayload::ContextMarker { position, marker } => {
            listener.on_context_marker(position, marker);
        }
        IpcEventPayload::AudioLevel { rms } => listener.on_audio_level(rms),
        IpcEventPayload::TranscriptProjection { json } => {
            match serde_json::from_str::<
                codescribe::presentation::transcript_bus::TranscriptBusEvidenceEvent,
            >(&json)
            {
                Ok(event) => listener.on_transcript_projection(
                    CsTranscriptProjectionEvent::from(&event),
                ),
                Err(error) => tracing::warn!(
                    %error,
                    "transcript projection transport rejected invalid Bus schema"
                ),
            }
        }
        IpcEventPayload::Engine(event) => match event {
            EngineEventWire::VadStart { .. } => listener.on_vad_active(true),
            EngineEventWire::VadEnd { .. } => listener.on_vad_active(false),
            EngineEventWire::SidebandEvidence { evidence } => {
                // Content-free timing evidence reaches the app boundary for
                // diagnostics, but there is deliberately no UI decoration or
                // transcript callback in W2-02. L3 already consumes the
                // permitted pause-only subset inside core.
                tracing::debug!(
                    sequence = evidence.sequence,
                    session = %evidence.range.session,
                    capture_epoch = evidence.range.capture_epoch,
                    sample_start = evidence.range.sample_start,
                    sample_end = evidence.range.sample_end,
                    provenance = ?evidence.provenance,
                    kind = ?evidence.evidence,
                    "Silero sideband evidence (non-text)"
                );
            }
            EngineEventWire::NoSpeech { reason } => listener.on_no_speech(reason),
            EngineEventWire::Preview { text, .. } => listener.on_preview(text),
            EngineEventWire::Correction {
                text,
                previous_text,
                ..
            } => listener.on_correction(text, previous_text),
            EngineEventWire::UtteranceFinal {
                utterance_id,
                text,
                avg_logprob,
                vad_speech_pct,
                confidence_flags,
                ..
            } => {
                let flags: Vec<String> = confidence_flags.iter().map(ToString::to_string).collect();
                listener.on_final(utterance_id, text, avg_logprob, vad_speech_pct, flags);
            }
            EngineEventWire::ReplaceRange {
                utterance_id,
                start,
                end,
                text,
                source,
            } => listener.on_replace_range(
                utterance_id,
                start as u64,
                end as u64,
                text,
                source.into(),
            ),
            EngineEventWire::InsertAnnotation {
                utterance_id,
                position,
                text,
                kind,
            } => listener.on_insert_annotation(
                utterance_id,
                position as u64,
                text,
                CsAnnotationKind::from(&kind),
            ),
            EngineEventWire::SessionFinalised {
                session_id,
                layer_summary,
            } => listener.on_session_finalised(session_id, CsLayerSummary::from(&layer_summary)),
            // Same class split as the matching arm in recording.rs: failures
            // reach `on_error`, quality receipts are log-only. The tray stays
            // as it is: a degraded-quality warning is not a dead backend.
            EngineEventWire::Warning { code, message } => {
                if codescribe_core::pipeline::contracts::warning_is_user_terminal(&code) {
                    listener.on_error(format!("{code}: {message}"));
                } else {
                    tracing::info!(code, message, "engine warning (receipt, not forwarded)");
                }
            }
            EngineEventWire::Drop { .. } | EngineEventWire::Stats { .. } => {}
        },
    }
}

/// Snapshot the registered overlay listener, cloned out of the lock.
fn current_listener() -> Option<Arc<dyn CsTranscriptionListener>> {
    shared_listener()
        .read()
        .unwrap_or_else(|e| e.into_inner())
        .as_ref()
        .map(Arc::clone)
}

/// True while an optimistic "preparing" overlay has been shown but no terminal
/// event (`on_recording_started` / `on_recording_stopped`) has resolved it yet.
///
/// The optimistic overlay (`optimistically_show_overlay`) is driven by a DIRECT
/// listener call, bypassing the controller's `StateChange` broadcast. The only
/// mechanism that later dismisses it is that broadcast — but
/// `set_state_with_broadcast` stays silent when the state does not change. Any
/// dispatch that shows "preparing" and returns to Idle WITHOUT a state
/// transition (quick hold-release cancel, start-failure reset, no-op re-check)
/// therefore orphans the overlay forever. This flag lets the post-dispatch
/// compensator emit exactly one terminal `on_recording_stopped` for those paths,
/// while the broadcast forwarder clears it so a genuine start/stop never
/// double-fires.
static PREPARING_PENDING: AtomicBool = AtomicBool::new(false);

/// Show the "preparing" overlay immediately on a start gesture, before the
/// controller has done any work, so the UI reacts at key-down latency instead of
/// after model/recorder setup.
///
/// Only fires for gestures that actually start a session, and only from `Idle` —
/// a mid-session event must not repaint a preparing state over live capture.
/// Arms [`PREPARING_PENDING`] first so the terminal half of the contract holds
/// even when the dispatch that follows never transitions state.
async fn optimistically_show_overlay(event: &HotkeyEvent) {
    let starts_redesign_overlay = matches!(
        event,
        HotkeyEvent::ToggleNormal
            | HotkeyEvent::ToggleRaw
            | HotkeyEvent::ToggleAssistive
            | HotkeyEvent::Hold {
                action: HoldAction::Down,
                ..
            }
    );
    if !starts_redesign_overlay {
        return;
    }
    if let Some(existing) = current_controller(&shared_controller())
        && existing.current_state().await != State::Idle
    {
        return;
    }
    if let Some(listener) = current_listener() {
        // Arm the compensator BEFORE the direct call so the terminal guarantee
        // holds even if the dispatch that follows never transitions state.
        PREPARING_PENDING.store(true, Ordering::Release);
        let indicator_mode = match event {
            HotkeyEvent::ToggleAssistive
            | HotkeyEvent::Hold {
                mode: HoldMode::Chat | HoldMode::Selection,
                ..
            } => BadgeMode::Assistive,
            HotkeyEvent::ToggleNormal | HotkeyEvent::ToggleRaw => BadgeMode::Toggle,
            _ => BadgeMode::Hold,
        };
        tray_status::set_tray_indicator_mode(indicator_mode);
        tray_status::update_tray_status(TrayStatus::Starting);
        listener.on_recording_preparing();
    }
}

/// Guarantee the terminal half of the "preparing" contract after a dispatch.
///
/// Run once after every `dispatch_hotkey_event` that may have shown an optimistic
/// overlay. If a "preparing" is still pending AND the controller did not end up
/// recording, the optimistic overlay was orphaned (no `StateChange` broadcast
/// will ever dismiss it) — emit the compensating terminal stop. If the controller
/// is recording (or finalising via `Busy`), the broadcast forwarder owns the
/// transition and we leave the flag for it to clear. The `swap` makes the stop
/// idempotent against a forwarder that already resolved the same "preparing".
async fn compensate_orphaned_preparing(controller: &Arc<RecordingController>) {
    if controller.current_state().await != State::Idle {
        // Recording/finalising: the StateChange broadcast drives preparing→started
        // and, later, →stopped. Nothing to compensate here.
        return;
    }
    if PREPARING_PENDING.swap(false, Ordering::AcqRel) {
        tray_status::update_tray_status(TrayStatus::Idle);
        if let Some(listener) = current_listener() {
            listener.on_recording_stopped();
        }
    }
}

/// Process-global hotkey runtime owner.
///
/// `start()` installs the native listener but creates `RecordingController`
/// lazily on the first real hotkey event. That keeps app launch/menu-open free
/// of `Config::load()` side effects while still routing hotkeys through the
/// real controller once the user intentionally invokes a shortcut.
#[derive(uniffi::Object, Default)]
pub struct CodescribeHotkeys {}

#[uniffi::export]
impl CodescribeHotkeys {
    /// Construct the hotkey facade and initialise logging. Creates no listener,
    /// controller or tap — `start()` owns that.
    #[uniffi::constructor]
    pub fn new() -> Self {
        codescribe::logging::init_logging();
        Self::default()
    }

    /// Start or replace the process-global hotkey listener.
    pub async fn start(&self) -> Result<(), CsError> {
        application_runtime::run(async move {
            // Install the process-wide macOS thermal observer once at runtime
            // bootstrap so STT duty-cycle throttling (core/stt/scheduler.rs) sees
            // real thermal pressure. Without this the scheduler always reads
            // ThermalLevel::Nominal and never backs off during hot/long sessions.
            // Idempotent: install_thermal_probe guards its own observer singleton.
            codescribe::os::thermal::install_thermal_probe();

            // Seed the hotkey detector atomics from persisted config so the
            // CGEventTap honours the user's saved mode bindings / cadence from
            // launch. The atomics otherwise hold only compile-time defaults, so
            // non-default bindings would never take effect. update_config re-applies
            // this on every later settings change for live-reload without restart.
            codescribe::os::hotkeys::apply_hotkey_config(
                &codescribe_core::config::Config::load_without_keychain(),
            );

            let (tx, rx) = unbounded::<HotkeyEvent>();
            let handle = tokio::runtime::Handle::current();
            // Publish the runtime handle so sync config-write surfaces can push fresh
            // settings into the live controller (refresh_live_controller_config).
            let _ = shared_runtime_handle().set(handle.clone());
            // Bridge the app-side voice-assistive delivery broadcast onto the Swift
            // AgentChat listener. Idempotent — a repeated start() does not stack a
            // second forwarder. The listener itself is registered separately via
            // `set_agent_delivery_listener` and may arrive before or after this.
            spawn_delivery_forwarder(handle.clone());
            let controller_store = shared_controller();

            // One consumer owns recording gesture order. Spawning one task per
            // event lets a quick HoldUp observe Idle before HoldDown has armed
            // the controller, then leaves the later Down without its release.
            // The unbounded sender is appropriate here: the native hotkey
            // channel is already unbounded and this queue holds tiny enums,
            // while the single consumer gives Down/Up FIFO semantics.
            let (recording_tx, mut recording_rx) =
                tokio::sync::mpsc::unbounded_channel::<HotkeyEvent>();
            let recording_controller_store = Arc::clone(&controller_store);
            let recording_controller_handle = handle.clone();
            handle.spawn(async move {
                while let Some(recording_event) = recording_rx.recv().await {
                    let controller = ensure_controller(
                        &recording_controller_store,
                        recording_controller_handle.clone(),
                    );
                    let dispatch =
                        dispatch_recording_with_capture_gate(recording_event, controller).await;
                    if let Err(error) = dispatch {
                        tray_status::update_tray_status(TrayStatus::Error);
                        notifications::notify("Codescribe", &error.to_string());
                        tracing::error!(%error, "Hotkey event dispatch failed");
                    }
                }
            });

            // Spawn the event-dispatch thread BEFORE bringing up the tap. It drains
            // `rx` for the lifetime of the retained sender, so it stays ready whether
            // the CGEventTap comes up now (permissions already granted) or later via
            // `rearm_after_permission_grant` after a first-run TCC grant. If it were
            // spawned only after a successful `install_global_hotkey_manager`, a
            // permission-less cold start would leave no consumer, and a later re-arm
            // would build a live tap whose events pile up in the channel undispatched.
            std::thread::spawn(move || {
                for event in rx {
                    let recording_tx = recording_tx.clone();
                    route_hotkey_event(
                        event,
                        current_app_action_listener(),
                        move |recording_event| {
                            if recording_tx.send(recording_event).is_err() {
                                tray_status::update_tray_status(TrayStatus::Error);
                                notifications::notify(
                                    "Codescribe",
                                    "Hotkey recording dispatcher stopped",
                                );
                            }
                        },
                        deliver_deferred_insert_and_notify,
                    );
                }
            });

            // Bring up the tap. On a permission-less first launch this returns an
            // error, but the sender is retained inside the hotkey service so a later
            // `rearm_after_permission_grant` can create the tap and feed the
            // already-running dispatch thread — no app restart required.
            hotkeys::install_global_hotkey_manager(tx.clone())
                .map_err(|msg| CsError::Recording { msg })?;

            Ok(())
        })
        .await?
    }

    /// Register the Swift overlay listener for the shared controller event stream.
    pub fn set_listener(&self, listener: Arc<dyn CsTranscriptionListener>) {
        let listener_store = shared_listener();
        let mut guard = listener_store.write().unwrap_or_else(|e| e.into_inner());
        *guard = Some(listener);
    }

    /// Register the Swift AgentChat listener that renders voice-assistive replies
    /// live. Process-global, so it takes effect for the delivery forwarder spawned
    /// in `start()` regardless of call order. Swift must keep a strong reference
    /// to the listener (UniFFI otherwise releases the foreign callback).
    pub fn set_agent_delivery_listener(&self, listener: Arc<dyn CsAgentDeliveryListener>) {
        set_delivery_listener(listener);
    }

    /// Register the Swift listener for no-payload application commands.
    pub fn set_app_action_listener(&self, listener: Arc<dyn CsAppActionListener>) {
        let store = shared_app_action_listener();
        let mut guard = store.write().unwrap_or_else(|e| e.into_inner());
        *guard = Some(listener);
    }

    /// Prompt-free warmup for the shared recording controller.
    ///
    /// This intentionally does not start recording. It front-loads the expensive
    /// local recorder/model setup after app launch so the first user-triggered
    /// dictation does not sit in the overlay's `starting` state for seconds.
    pub async fn prewarm_recording(&self) -> Result<(), CsError> {
        application_runtime::run(async move {
            let _ = ensure_controller(&shared_controller(), tokio::runtime::Handle::current());
            // Warm the ACTIVE engine the router will actually use (Apple SpeechAnalyzer
            // on macOS 26+, Candle on fallback/older macOS) — not a hardcoded Candle
            // singleton. `prewarm_active_engine` also runs a synthetic warmup inference,
            // so the first user dictation pays neither model-load nor Metal
            // kernel-compilation latency. Idempotent; safe to race the controller's own
            // background prewarm.
            tokio::task::spawn_blocking(codescribe::stt::prewarm_active_engine)
                .await
                .map_err(|error| CsError::Recording {
                    msg: format!("STT prewarm task failed: {error}"),
                })?
                .map_err(|error| CsError::Recording {
                    msg: format!("STT prewarm failed: {error}"),
                })?;
            Ok(())
        })
        .await?
    }

    /// Start the same toggle recording flow used by the default hotkey.
    pub async fn start_recording(&self) -> Result<(), CsError> {
        application_runtime::run(async move {
            start_recording_with_event(HotkeyEvent::ToggleNormal).await
        })
        .await?
    }

    /// Start the same toggle flow in the assistive lane. Overlay owns this
    /// route — the Agent composer mic is a separate, UI-initiated capture.
    pub async fn start_assistive_recording(&self) -> Result<(), CsError> {
        application_runtime::run(async move {
            start_recording_with_event(HotkeyEvent::ToggleAssistive).await
        })
        .await?
    }

    /// Overlay Retranscribe: `hq:` / `cloud:` prefixes pick the pass.
    /// Bare paths are a Full HQ file pass.
    pub async fn transcribe_file(&self, path: String) -> Result<CsTranscription, CsError> {
        application_runtime::run(
            async move { crate::recording::transcribe_session_file(path).await },
        )
        .await?
    }

    /// Stable path of the last retained session WAV, if it exists.
    pub fn last_session_audio_path(&self) -> Option<String> {
        crate::recording::last_session_audio_path()
    }

    /// Stop the active legacy-controller recording flow, if one is live.
    pub async fn stop_recording(&self) -> Result<(), CsError> {
        application_runtime::run(async move {
            let Some(controller) = current_controller(&shared_controller()) else {
                return Ok(());
            };
            controller
                .stop_recording_from_external_surface()
                .await
                .map_err(|error| CsError::Recording {
                    msg: error.to_string(),
                })
        })
        .await?
    }

    /// Forward a macOS sleep/wake boundary to the active recorder, if any.
    ///
    /// Querying this surface never constructs the shared controller. The host
    /// notification callback can therefore remain a cheap no-op while idle and
    /// cannot surprise-load a model or start a provider.
    pub async fn note_sleep_wake(&self) -> bool {
        match application_runtime::run(async move {
            let Some(controller) = current_controller(&shared_controller()) else {
                return false;
            };
            controller.note_sleep_wake().await
        })
        .await
        {
            Ok(reached) => reached,
            Err(error) => {
                tracing::error!(%error, "sleep/wake runtime dispatch failed");
                false
            }
        }
    }

    /// True while the shared controller is in an active recording/conversation state.
    pub async fn is_recording(&self) -> bool {
        match application_runtime::run(async move {
            let Some(controller) = current_controller(&shared_controller()) else {
                return false;
            };
            matches!(
                controller.current_state().await,
                codescribe::controller::State::RecHold
                    | codescribe::controller::State::RecToggle
                    | codescribe::controller::State::Conversation
            )
        })
        .await
        {
            Ok(recording) => recording,
            Err(error) => {
                tracing::error!(%error, "recording-state runtime dispatch failed");
                false
            }
        }
    }

    /// True when the configured formatting provider can handle a user-triggered
    /// overlay format action.
    pub fn is_formatting_available(&self) -> bool {
        Config::load_runtime_snapshot().is_ok_and(|runtime_settings| {
            codescribe::ai_formatting::is_formatting_available(
                runtime_settings.llm_lanes().formatting(),
            )
        })
    }

    /// Format editable overlay text after recording stops.
    pub async fn format_text(
        &self,
        text: String,
        language: Option<CsLanguage>,
    ) -> Result<String, CsError> {
        application_runtime::run(async move {
            let runtime_settings =
                Config::load_runtime_snapshot().map_err(|error| CsError::Config {
                    msg: error.to_string(),
                })?;
            let language = language.map(|l| l.as_code().to_string());
            let result = codescribe::ai_formatting::format_text_with_status(
                &text,
                language.as_deref(),
                false,
                runtime_settings.llm_lanes().formatting(),
                None,
            )
            .await;
            if result.text.trim().is_empty() {
                Ok(text)
            } else {
                Ok(result.text)
            }
        })
        .await?
    }

    /// Format overlay text through an explicitly selected one-shot level
    /// (`correction` / `smart` / `max`). Never reads or writes the persisted
    /// Auto Format policy; `off` is rejected — a manual action must act.
    pub async fn format_text_for_level(
        &self,
        text: String,
        language: Option<CsLanguage>,
        level: String,
    ) -> Result<String, CsError> {
        application_runtime::run(async move {
            let runtime_settings =
                Config::load_runtime_snapshot().map_err(|error| CsError::Config {
                    msg: error.to_string(),
                })?;
            let policy = FormattingPolicy::parse(&level).map_err(|error| CsError::Config {
                msg: error.to_string(),
            })?;
            if policy == FormattingPolicy::Off {
                return Err(CsError::Config {
                    msg: "manual format level cannot be 'off'".to_string(),
                });
            }
            let language = language.map(|l| l.as_code().to_string());
            let result = codescribe::ai_formatting::format_text_with_status_for_policy(
                &text,
                language.as_deref(),
                policy,
                runtime_settings.llm_lanes().formatting(),
            )
            .await;
            if result.text.trim().is_empty() {
                Ok(text)
            } else {
                Ok(result.text)
            }
        })
        .await?
    }

    /// Paste edited overlay text back into the app that was frontmost before the
    /// overlay. The result includes delivery truth and the app names observed
    /// at the exact delivery boundary so Swift can explain every degradation.
    pub async fn paste_text(&self, text: String) -> Result<CsPasteResult, CsError> {
        application_runtime::run(async move {
            let controller =
                ensure_controller(&shared_controller(), tokio::runtime::Handle::current());
            controller
                .paste_text_from_overlay(text)
                .await
                .map(CsPasteResult::from)
                .map_err(|error| CsError::Recording {
                    msg: error.to_string(),
                })
        })
        .await?
    }

    /// Arm an edited overlay transcript directly when Swift knows the caret is
    /// still inside Codescribe. The controller owns tagging and the W1-A copy
    /// fallback when Paste Here registration is unavailable.
    pub async fn defer_text(&self, text: String) -> Result<CsPasteResult, CsError> {
        application_runtime::run(async move {
            let controller =
                ensure_controller(&shared_controller(), tokio::runtime::Handle::current());
            controller
                .defer_text_from_overlay(text)
                .await
                .map(CsPasteResult::from)
                .map_err(|error| CsError::Recording {
                    msg: error.to_string(),
                })
        })
        .await?
    }

    /// Copy the tagged transcript to the clipboard without a synthetic paste.
    /// Swift calls this when the caret already sits inside Codescribe, where a
    /// synthetic Cmd+V would paste the transcript back into the overlay itself.
    pub async fn copy_text_tagged(&self, text: String) -> Result<(), CsError> {
        application_runtime::run(async move {
            let controller =
                ensure_controller(&shared_controller(), tokio::runtime::Handle::current());
            controller
                .copy_text_from_overlay(text)
                .await
                .map_err(|error| CsError::Recording {
                    msg: error.to_string(),
                })
        })
        .await?
    }

    /// Name of the app latched for the current overlay session, if known.
    /// Read-only: the paste path keeps owning target activation and delivery.
    pub async fn paste_target_app_name(&self) -> Option<String> {
        match application_runtime::run(async move {
            let controller = current_controller(&shared_controller())?;
            normalize_paste_target_app_name(controller.paste_target_app_name().await)
        })
        .await
        {
            Ok(target) => target,
            Err(error) => {
                tracing::error!(%error, "paste-target runtime dispatch failed");
                None
            }
        }
    }

    /// Deliver an assistive transcript from the editable overlay. The
    /// controller attaches the trigger-time selection and accepts this once.
    pub async fn send_assistive_transcript(&self, text: String) -> Result<bool, CsError> {
        application_runtime::run(async move {
            let controller =
                ensure_controller(&shared_controller(), tokio::runtime::Handle::current());
            controller
                .deliver_pending_assistive_transcript(text)
                .await
                .map_err(|error| CsError::Recording {
                    msg: error.to_string(),
                })
        })
        .await?
    }

    /// Stop the global hotkey listener if it is active.
    pub fn stop(&self) {
        hotkeys::shutdown_global_hotkey_manager();
    }

    /// True once the listener is installed and owned by this process.
    pub fn is_active(&self) -> bool {
        hotkeys::is_global_hotkey_manager_active()
    }

    /// Cancel the controller-owned voice-assistive Agent turn correlated by the
    /// delivery thread id. This registry is independent of the controller's
    /// long-held runtime mutex, so the synchronous Swift Stop action cannot block
    /// behind provider or tool work.
    pub fn cancel_voice_turn(&self, thread_id: String) -> bool {
        codescribe::agent_delivery::cancel_agent_delivery_turn(&thread_id)
    }

    /// Publish the Agent UI's current thread selection as the voice-assistive
    /// routing target (operator contract 2026-08-13: dictation goes to the
    /// thread the user is looking at; a new thread only via an explicit
    /// "+ New thread"). `None` = the selection is a not-yet-persisted thread,
    /// so the next assistive turn mints a fresh one.
    pub fn set_assistive_target_thread(&self, backend_id: Option<String>) {
        codescribe::controller::set_assistive_target_thread(backend_id);
    }
}

/// Honest outcome of the overlay Insert action, mirrored to Swift so the UI
/// can tell the user when the self-paste guard degraded a paste to a tagged
/// clipboard copy.
#[derive(uniffi::Enum, Debug, Clone, Copy, PartialEq, Eq)]
pub enum CsPasteOutcome {
    Pasted,
    CopiedToClipboard,
    AccessibilityPermissionNeeded,
    DeferredInsertArmed,
    Noop,
}

impl From<codescribe::controller::OverlayPasteDelivery> for CsPasteOutcome {
    /// Map core overlay paste delivery into the UniFFI `CsPasteOutcome` enum.
    fn from(value: codescribe::controller::OverlayPasteDelivery) -> Self {
        match value {
            codescribe::controller::OverlayPasteDelivery::Pasted => Self::Pasted,
            codescribe::controller::OverlayPasteDelivery::CopiedToClipboard => {
                Self::CopiedToClipboard
            }
            codescribe::controller::OverlayPasteDelivery::AccessibilityPermissionNeeded => {
                Self::AccessibilityPermissionNeeded
            }
            codescribe::controller::OverlayPasteDelivery::DeferredInsertArmed => {
                Self::DeferredInsertArmed
            }
            codescribe::controller::OverlayPasteDelivery::Noop => Self::Noop,
        }
    }
}

/// Full delivery truth for one overlay Insert, including the app names observed
/// at the exact delivery boundary and the Paste Here shortcut (or the reason it
/// was unavailable), so Swift can explain any degradation instead of guessing.
#[derive(uniffi::Record, Debug, Clone, PartialEq, Eq)]
pub struct CsPasteResult {
    pub outcome: CsPasteOutcome,
    pub target_app_name: Option<String>,
    pub frontmost_app_name: Option<String>,
    pub deferred_insert_shortcut: Option<String>,
    pub deferred_insert_failure: Option<String>,
}

impl From<codescribe::controller::OverlayPasteResult> for CsPasteResult {
    /// Map core overlay paste result (delivery + app names) into FFI form.
    fn from(value: codescribe::controller::OverlayPasteResult) -> Self {
        Self {
            outcome: value.delivery.into(),
            target_app_name: value.target_app_name,
            frontmost_app_name: value.frontmost_app_name,
            deferred_insert_shortcut: value.deferred_insert_shortcut,
            deferred_insert_failure: value.deferred_insert_failure,
        }
    }
}

/// Run a UI-initiated recording gesture through the SAME capture gate and
/// dispatch path a real hotkey takes, so an FFI start can never bypass the
/// ownership rules the tap path enforces.
async fn start_recording_with_event(event: HotkeyEvent) -> Result<(), CsError> {
    let controller = ensure_controller(&shared_controller(), tokio::runtime::Handle::current());
    dispatch_recording_with_capture_gate(event, controller)
        .await
        .map_err(|error| CsError::Recording {
            msg: error.to_string(),
        })
}

/// Wrap a recording dispatch in the one-controller capture lifecycle: claim on
/// a session-starting event, dispatch, compensate an orphaned "preparing", and
/// release ownership once the controller is back at `Idle`.
///
/// The claim happens BEFORE any controller work so two racing gestures cannot
/// both believe they started a session.
async fn dispatch_recording_with_capture_gate(
    event: HotkeyEvent,
    controller: Arc<RecordingController>,
) -> anyhow::Result<()> {
    let state_before = controller.current_state().await;
    let starts_capture = state_before == State::Idle && event_can_start_capture(&event);
    if starts_capture {
        CAPTURE_OWNER
            .compare_exchange(
                CAPTURE_OWNER_NONE,
                CAPTURE_OWNER_CONTROLLER,
                Ordering::AcqRel,
                Ordering::Acquire,
            )
            .map_err(|_| anyhow::anyhow!("Another transcription capture is already starting"))?;
    }

    optimistically_show_overlay(&event).await;
    let dispatch = dispatch_recording_hotkey_event(event, Arc::clone(&controller)).await;
    compensate_orphaned_preparing(&controller).await;
    if controller.current_state().await == State::Idle {
        let _ = CAPTURE_OWNER.compare_exchange(
            CAPTURE_OWNER_CONTROLLER,
            CAPTURE_OWNER_NONE,
            Ordering::AcqRel,
            Ordering::Acquire,
        );
    }
    dispatch
}

/// Lower a `HotkeyEvent` into the `HotkeyInput` vocabulary the recording
/// controller state machine speaks.
///
/// UI-only commands are `unreachable!` here on purpose: `route_hotkey_event`
/// owns that split, and reaching this point with one means the routing contract
/// was violated rather than a user gesture being unusual.
async fn dispatch_recording_hotkey_event(
    event: HotkeyEvent,
    controller: Arc<RecordingController>,
) -> anyhow::Result<()> {
    match event {
        HotkeyEvent::ShowAgent | HotkeyEvent::InsertHere => {
            unreachable!("UI-only commands must be routed before recording dispatch")
        }
        HotkeyEvent::Hold { action, mode } => {
            let mapped_action = match action {
                HoldAction::Down => HotkeyAction::Down,
                HoldAction::Up => HotkeyAction::Up,
            };
            let input = HotkeyInput {
                key_type: HotkeyType::Hold,
                action: mapped_action,
                assistive: !matches!(mode, HoldMode::Raw),
                hold_mode: mode,
                force_raw: false,
                force_ai: false,
            };
            controller.handle_hotkey_event(input).await?;
        }
        HotkeyEvent::HoldUpdate { mode } => {
            let input = HotkeyInput {
                key_type: HotkeyType::Hold,
                action: HotkeyAction::Press,
                assistive: !matches!(mode, HoldMode::Raw),
                hold_mode: mode,
                force_raw: false,
                force_ai: false,
            };
            controller.handle_hotkey_event(input).await?;
        }
        HotkeyEvent::AttachSelection => {
            controller.attach_hold_selection().await?;
        }
        HotkeyEvent::ToggleNormal => {
            let input = HotkeyInput {
                key_type: HotkeyType::Toggle,
                action: HotkeyAction::Press,
                assistive: false,
                hold_mode: HoldMode::Raw,
                force_raw: false,
                force_ai: true,
            };
            controller.handle_hotkey_event(input).await?;
        }
        HotkeyEvent::ToggleRaw => {
            let input = HotkeyInput {
                key_type: HotkeyType::Toggle,
                action: HotkeyAction::Press,
                assistive: false,
                hold_mode: HoldMode::Raw,
                force_raw: true,
                force_ai: false,
            };
            controller.handle_hotkey_event(input).await?;
        }
        HotkeyEvent::ToggleAssistive => {
            let input = HotkeyInput {
                key_type: HotkeyType::Toggle,
                action: HotkeyAction::Press,
                assistive: true,
                hold_mode: HoldMode::Raw,
                force_raw: false,
                force_ai: false,
            };
            controller.handle_hotkey_event(input).await?;
        }
        HotkeyEvent::DoubleTapBlocked { gesture, reason } => {
            // Detector is the single owner of the stable
            // `blocked_double_tap gesture=… reason=…` INFO line (W11-C).
            // Bridge must not re-emit that token — only optional human prose.
            let body = format!(
                "{} was detected, but {}.",
                gesture.label(),
                reason.message()
            );
            tracing::debug!("Hotkey double-tap blocked: {body}");
            let _ = reason; // keep reason available for future Diagnostics UI
        }
    }

    Ok(())
}

#[cfg(test)]
mod application_shutdown_tests {
    use super::*;
    use serial_test::serial;

    #[test]
    #[serial]
    fn shutdown_releases_controller_capture_ownership() {
        CAPTURE_OWNER.store(CAPTURE_OWNER_CONTROLLER, Ordering::SeqCst);
        release_capture_ownership_for_shutdown();
        assert_eq!(
            CAPTURE_OWNER.load(Ordering::SeqCst),
            CAPTURE_OWNER_NONE,
            "application shutdown must never leave microphone ownership latched"
        );
    }

    #[tokio::test]
    async fn shutdown_recording_wait_is_bounded() {
        let error = await_recording_shutdown(
            std::future::pending::<Result<(), CsError>>(),
            Duration::from_millis(5),
        )
        .await
        .expect_err("pending stop must time out");
        let CsError::Recording { msg } = error else {
            panic!("shutdown timeout must be a recording error");
        };
        assert!(msg.contains("timed out"), "{msg}");
    }
}

/// One-shot manual format levels: unknown levels and `off` are rejected (a
/// manual action must act), while legacy aliases still normalize through the
/// same `FormattingPolicy` owner.
#[cfg(test)]
mod format_level_tests {
    use super::*;

    /// Unknown level strings must fail config-side, not silently no-op.
    #[tokio::test]
    async fn format_text_for_level_rejects_unknown_level() {
        let hotkeys = CodescribeHotkeys::default();
        let result = hotkeys
            .format_text_for_level("hello".to_string(), None, "mega".to_string())
            .await;
        assert!(matches!(result, Err(CsError::Config { .. })));
    }

    /// Manual format must act: `off` is rejected rather than a silent pass-through.
    #[tokio::test]
    async fn format_text_for_level_rejects_off() {
        let hotkeys = CodescribeHotkeys::default();
        let result = hotkeys
            .format_text_for_level("hello".to_string(), None, "off".to_string())
            .await;
        assert!(matches!(result, Err(CsError::Config { .. })));
    }

    /// Legacy aliases (e.g. `creative` → Max) still normalize via FormattingPolicy.
    #[tokio::test]
    async fn format_text_for_level_accepts_legacy_alias_shape() {
        // Aliases normalize through the same FormattingPolicy owner as C01;
        // "creative" must map to Max, not fail. No provider is configured in
        // tests, so the formatter falls back to returning usable text without
        // any network call.
        let hotkeys = CodescribeHotkeys::default();
        let result = hotkeys
            .format_text_for_level("hi".to_string(), None, "creative".to_string())
            .await;
        assert!(result.is_ok());
    }
}

/// Recording-dispatch side effects — notably that a blocked double-tap stays
/// silent on the tray instead of publishing a conflict the detector already owns.
#[cfg(test)]
mod dispatch_tests {
    use super::*;
    use codescribe::os::hotkeys::{DoubleTapBlockReason, DoubleTapGesture};

    /// Detector already owns blocked-double-tap tray truth; bridge stays silent.
    #[tokio::test]
    #[serial_test::serial]
    async fn blocked_double_tap_does_not_publish_tray_conflict() {
        tray_status::update_tray_status(TrayStatus::Idle);

        let controller = Arc::new(RecordingController::new_without_keychain());
        dispatch_recording_hotkey_event(
            HotkeyEvent::DoubleTapBlocked {
                gesture: DoubleTapGesture::LeftOption,
                reason: DoubleTapBlockReason::ModifierComboActive,
            },
            controller,
        )
        .await
        .expect("blocked double-tap dispatch should not fail");

        assert_eq!(tray_status::current_tray_status(), TrayStatus::Idle);
    }
}

/// The routing contract of [`route_hotkey_event`]: all capture modes reach one
/// recording callback; Agent UI notification carries no audio/transcript data.
#[cfg(test)]
mod app_action_tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;

    /// Test double that counts UI-only Agent summons.
    struct CountingAppActionListener {
        show_agent_calls: AtomicUsize,
    }

    impl CsAppActionListener for CountingAppActionListener {
        /// Count ShowAgent UI callbacks (no recording side effects).
        fn on_show_agent(&self) {
            self.show_agent_calls.fetch_add(1, Ordering::SeqCst);
        }
    }

    /// Every session-starting gesture is recognized by the same capture gate.
    #[test]
    fn dictation_agent_and_assistive_all_start_shared_capture() {
        for event in [
            HotkeyEvent::ToggleNormal,
            HotkeyEvent::ToggleAssistive,
            HotkeyEvent::Hold {
                action: HoldAction::Down,
                mode: HoldMode::Raw,
            },
            HotkeyEvent::Hold {
                action: HoldAction::Down,
                mode: HoldMode::Chat,
            },
        ] {
            assert!(
                event_can_start_capture(&event),
                "missing shared start for {event:?}"
            );
        }
    }

    /// ShowAgent remains UI-only; recording gestures all reach the same callback.
    #[test]
    fn agent_notification_does_not_own_capture_or_transcript_payload() {
        let listener = Arc::new(CountingAppActionListener {
            show_agent_calls: AtomicUsize::new(0),
        });
        let recording_calls = Arc::new(AtomicUsize::new(0));
        let recording_calls_for_route = Arc::clone(&recording_calls);

        route_hotkey_event(
            HotkeyEvent::ShowAgent,
            Some(listener.clone()),
            move |_| {
                recording_calls_for_route.fetch_add(1, Ordering::SeqCst);
            },
            || panic!("show agent must not dispatch deferred insert"),
        );

        assert_eq!(listener.show_agent_calls.load(Ordering::SeqCst), 1);
        assert_eq!(recording_calls.load(Ordering::SeqCst), 0);

        let recording_calls_for_route = Arc::clone(&recording_calls);
        route_hotkey_event(
            HotkeyEvent::ToggleNormal,
            Some(listener.clone()),
            move |_| {
                recording_calls_for_route.fetch_add(1, Ordering::SeqCst);
            },
            || panic!("recording command must not dispatch deferred insert"),
        );
        assert_eq!(listener.show_agent_calls.load(Ordering::SeqCst), 1);
        assert_eq!(recording_calls.load(Ordering::SeqCst), 1);

        let recording_calls_for_route = Arc::clone(&recording_calls);
        route_hotkey_event(
            HotkeyEvent::ToggleAssistive,
            Some(listener.clone()),
            move |_| {
                recording_calls_for_route.fetch_add(1, Ordering::SeqCst);
            },
            || panic!("assistive command must not dispatch deferred insert"),
        );
        assert_eq!(listener.show_agent_calls.load(Ordering::SeqCst), 2);
        assert_eq!(recording_calls.load(Ordering::SeqCst), 2);

        let deferred_calls = Arc::new(AtomicUsize::new(0));
        let deferred_calls_for_route = Arc::clone(&deferred_calls);
        route_hotkey_event(
            HotkeyEvent::InsertHere,
            Some(listener.clone()),
            |_| panic!("deferred insert must not enter recording dispatch"),
            move || {
                deferred_calls_for_route.fetch_add(1, Ordering::SeqCst);
            },
        );
        assert_eq!(deferred_calls.load(Ordering::SeqCst), 1);
        assert_eq!(listener.show_agent_calls.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn mid_hold_attach_does_not_target_agent_or_claim_capture() {
        assert!(!event_can_start_capture(&HotkeyEvent::AttachSelection));
        assert!(!event_targets_agent_ui(&HotkeyEvent::AttachSelection));
        assert!(!event_targets_agent_ui(&HotkeyEvent::HoldUpdate {
            mode: HoldMode::Chat,
        }));
        assert!(!event_targets_agent_ui(&HotkeyEvent::Hold {
            action: HoldAction::Up,
            mode: HoldMode::Chat,
        }));

        let listener = Arc::new(CountingAppActionListener {
            show_agent_calls: AtomicUsize::new(0),
        });
        let recording_calls = Arc::new(AtomicUsize::new(0));
        let recording_calls_for_route = Arc::clone(&recording_calls);
        route_hotkey_event(
            HotkeyEvent::AttachSelection,
            Some(listener.clone()),
            move |_| {
                recording_calls_for_route.fetch_add(1, Ordering::SeqCst);
            },
            || panic!("attach must not dispatch deferred insert"),
        );
        assert_eq!(listener.show_agent_calls.load(Ordering::SeqCst), 0);
        assert_eq!(recording_calls.load(Ordering::SeqCst), 1);

        let recording_calls_for_route = Arc::clone(&recording_calls);
        route_hotkey_event(
            HotkeyEvent::HoldUpdate {
                mode: HoldMode::Chat,
            },
            Some(listener.clone()),
            move |_| {
                recording_calls_for_route.fetch_add(1, Ordering::SeqCst);
            },
            || panic!("hold update must not dispatch deferred insert"),
        );
        assert_eq!(listener.show_agent_calls.load(Ordering::SeqCst), 0);
        assert_eq!(recording_calls.load(Ordering::SeqCst), 2);
    }
}

// ===========================================================================
// Mode-binding configuration surface (B0)
//
// The hotkey ENGINE — mode-first bindings, seeded at launch and live-reloaded on
// every settings write — already exists after Wave A3. What was missing is a
// Settings editor: read the current per-mode bindings, propose a change, validate
// it for conflicts, and persist it so the running CGEventTap honours it WITHOUT
// an app restart.
//
// Writes go through the core's first-class `UserSettings::set_mode_binding`
// (mode bindings are NOT a `save_to_env` router key, so `update_config` can't
// carry them), then re-apply the hotkey atomics via the SAME `apply_hotkey_config`
// path `CodescribeConfig::update_config` uses — preserving A3 live-reload.
// Conflict validation reuses the revived `shortcut_registry` gem
// (`detect_hotkey_conflicts` + the informational `fn_tap_intercept_note`).
// ===========================================================================

/// The three first-class work modes, mirrored from `codescribe_core::config::WorkMode`.
#[derive(uniffi::Enum, Debug, Clone, Copy, PartialEq, Eq)]
pub enum CsWorkMode {
    Dictation,
    Formatting,
    Assistive,
}

impl From<WorkMode> for CsWorkMode {
    /// Core `WorkMode` → UniFFI `CsWorkMode` (closed three-mode set).
    fn from(mode: WorkMode) -> Self {
        match mode {
            WorkMode::Dictation => CsWorkMode::Dictation,
            WorkMode::Formatting => CsWorkMode::Formatting,
            WorkMode::Assistive => CsWorkMode::Assistive,
        }
    }
}

impl From<CsWorkMode> for WorkMode {
    /// UniFFI `CsWorkMode` → core `WorkMode` for settings persistence.
    fn from(mode: CsWorkMode) -> Self {
        match mode {
            CsWorkMode::Dictation => WorkMode::Dictation,
            CsWorkMode::Formatting => WorkMode::Formatting,
            CsWorkMode::Assistive => WorkMode::Assistive,
        }
    }
}

/// A normalized gesture a work mode can bind to, mirrored from
/// `codescribe_core::config::ShortcutBinding`. This is a CLOSED set — the Settings
/// picker offers exactly these, matching `docs/HOTKEYS_CONTRACT.md`.
#[derive(uniffi::Enum, Debug, Clone, Copy, PartialEq, Eq)]
pub enum CsShortcutBinding {
    Disabled,
    HoldFn,
    HoldCtrl,
    HoldCtrlAlt,
    HoldCtrlShift,
    HoldCtrlCmd,
    DoubleCtrl,
    DoubleLeftOption,
    DoubleRightOption,
}

impl From<ShortcutBinding> for CsShortcutBinding {
    /// Core `ShortcutBinding` → UniFFI picker enum (closed gesture set).
    fn from(binding: ShortcutBinding) -> Self {
        match binding {
            ShortcutBinding::Disabled => CsShortcutBinding::Disabled,
            ShortcutBinding::HoldFn => CsShortcutBinding::HoldFn,
            ShortcutBinding::HoldCtrl => CsShortcutBinding::HoldCtrl,
            ShortcutBinding::HoldCtrlAlt => CsShortcutBinding::HoldCtrlAlt,
            ShortcutBinding::HoldCtrlShift => CsShortcutBinding::HoldCtrlShift,
            ShortcutBinding::HoldCtrlCmd => CsShortcutBinding::HoldCtrlCmd,
            ShortcutBinding::DoubleCtrl => CsShortcutBinding::DoubleCtrl,
            ShortcutBinding::DoubleLeftOption => CsShortcutBinding::DoubleLeftOption,
            ShortcutBinding::DoubleRightOption => CsShortcutBinding::DoubleRightOption,
        }
    }
}

impl From<CsShortcutBinding> for ShortcutBinding {
    /// UniFFI `CsShortcutBinding` → core binding for validation and save.
    fn from(binding: CsShortcutBinding) -> Self {
        match binding {
            CsShortcutBinding::Disabled => ShortcutBinding::Disabled,
            CsShortcutBinding::HoldFn => ShortcutBinding::HoldFn,
            CsShortcutBinding::HoldCtrl => ShortcutBinding::HoldCtrl,
            CsShortcutBinding::HoldCtrlAlt => ShortcutBinding::HoldCtrlAlt,
            CsShortcutBinding::HoldCtrlShift => ShortcutBinding::HoldCtrlShift,
            CsShortcutBinding::HoldCtrlCmd => ShortcutBinding::HoldCtrlCmd,
            CsShortcutBinding::DoubleCtrl => ShortcutBinding::DoubleCtrl,
            CsShortcutBinding::DoubleLeftOption => ShortcutBinding::DoubleLeftOption,
            CsShortcutBinding::DoubleRightOption => ShortcutBinding::DoubleRightOption,
        }
    }
}

/// One work mode's current binding, with display labels sourced from the core so
/// the Settings UI never re-invents copy that lives in `HOTKEYS_CONTRACT`.
#[derive(uniffi::Record, Debug, Clone)]
pub struct CsModeBinding {
    pub mode: CsWorkMode,
    pub mode_label: String,
    pub mode_description: String,
    pub binding: CsShortcutBinding,
    pub binding_label: String,
}

/// One selectable gesture for the Settings picker (id + display label).
#[derive(uniffi::Record, Debug, Clone)]
pub struct CsBindingOption {
    pub binding: CsShortcutBinding,
    pub label: String,
}

/// One detected conflict for a candidate binding set. `blocking` conflicts must be
/// resolved before a save is allowed; non-blocking entries are informational
/// (e.g. the macOS Fn-tap intercept note).
#[derive(uniffi::Record, Debug, Clone)]
pub struct CsHotkeyConflict {
    pub gesture_label: String,
    pub message: String,
    pub blocking: bool,
}

/// Closed enumeration of work modes used by Settings round-trips and tests.
const ALL_WORK_MODES: [WorkMode; 3] = [
    WorkMode::Dictation,
    WorkMode::Formatting,
    WorkMode::Assistive,
];

/// Closed gesture set offered by the Settings picker (`HOTKEYS_CONTRACT`).
const ALL_SHORTCUT_BINDINGS: [ShortcutBinding; 9] = [
    ShortcutBinding::Disabled,
    ShortcutBinding::HoldFn,
    ShortcutBinding::HoldCtrl,
    ShortcutBinding::HoldCtrlAlt,
    ShortcutBinding::HoldCtrlShift,
    ShortcutBinding::HoldCtrlCmd,
    ShortcutBinding::DoubleCtrl,
    ShortcutBinding::DoubleLeftOption,
    ShortcutBinding::DoubleRightOption,
];

/// Pair a mode with its binding and attach the display copy sourced from the
/// core, so the Settings UI never re-invents labels that live in
/// `docs/HOTKEYS_CONTRACT.md`.
fn build_mode_binding(mode: WorkMode, binding: ShortcutBinding) -> CsModeBinding {
    CsModeBinding {
        mode: mode.into(),
        mode_label: mode.label().to_string(),
        mode_description: mode.description().to_string(),
        binding: binding.into(),
        binding_label: binding.label().to_string(),
    }
}

/// Re-seed the live hotkey detector atomics from persisted settings after a
/// binding write. Identical to `CodescribeConfig::update_config`'s reload step, so
/// mode-binding edits take effect on the running CGEventTap without a restart.
fn reload_hotkey_runtime_after_write() {
    // Binding-only reload: never populate the Keychain (would prompt for a
    // password on every mode-binding save even though bindings need none).
    hotkeys::apply_hotkey_config(&Config::load_without_keychain());
}

/// Decide whether a permission-grant re-arm should rebuild the CGEventTap.
///
/// Rebuild only when the tap is NOT already live (dedup: it is process-global and
/// survives TCC re-checks, so re-arming a running tap would needlessly tear it
/// down) AND both permissions that gate `CGEventTapCreate` are granted (otherwise
/// the rebuild would fail again and churn). Pure so it is unit-testable without a
/// live tap or real TCC grants.
fn should_rearm_hotkey_tap(
    already_active: bool,
    accessibility: PermissionStatus,
    input_monitoring: PermissionStatus,
) -> bool {
    !already_active
        && accessibility == PermissionStatus::Granted
        && input_monitoring == PermissionStatus::Granted
}

#[uniffi::export]
impl CodescribeHotkeys {
    /// Re-arm the global CGEventTap after a first-run permission grant, without
    /// an app restart. The tap reads Accessibility / Input Monitoring only when
    /// it is created, so a grant made in System Settings after launch otherwise
    /// leaves every hotkey dead until the app is relaunched (the "TCC fresh-grant
    /// dance").
    ///
    /// Idempotent and safe to call on every permission Refresh: a no-op when the
    /// tap is already live (dedup — CGEventTap survives TCC re-checks) or when the
    /// two gating permissions are not both granted yet. Returns whether hotkeys
    /// are live after the call.
    pub fn rearm_after_permission_grant(&self) -> bool {
        let already_active = hotkeys::is_global_hotkey_manager_active();
        if !should_rearm_hotkey_tap(
            already_active,
            check_accessibility(),
            check_input_monitoring(),
        ) {
            return already_active;
        }
        match hotkeys::refresh_global_hotkey_manager() {
            Ok(()) => true,
            Err(error) => {
                eprintln!("Hotkey re-arm after permission grant failed: {error}");
                false
            }
        }
    }

    /// Current per-mode bindings (Dictation / Formatting / Assistive), normalized
    /// against defaults so every mode is always present. Reads on-disk truth.
    pub fn get_mode_bindings(&self) -> Vec<CsModeBinding> {
        let settings = UserSettings::load();
        ALL_WORK_MODES
            .iter()
            .map(|&mode| build_mode_binding(mode, settings.mode_binding_for(mode)))
            .collect()
    }

    /// The closed set of gestures a mode can bind to, with display labels. Drives
    /// the Settings picker (no free-form key capture — the binding space is a
    /// fixed enum, see `HOTKEYS_CONTRACT`).
    pub fn available_bindings(&self) -> Vec<CsBindingOption> {
        ALL_SHORTCUT_BINDINGS
            .iter()
            .map(|&binding| CsBindingOption {
                binding: binding.into(),
                label: binding.label().to_string(),
            })
            .collect()
    }

    /// Persist one mode's binding through the core's canonical `set_mode_binding`
    /// contract, then live-reload the detector atomics.
    pub fn set_mode_binding(
        &self,
        mode: CsWorkMode,
        binding: CsShortcutBinding,
    ) -> Result<(), CsError> {
        if mode == CsWorkMode::Assistive
            && matches!(
                binding,
                CsShortcutBinding::HoldFn
                    | CsShortcutBinding::HoldCtrl
                    | CsShortcutBinding::HoldCtrlAlt
                    | CsShortcutBinding::HoldCtrlShift
                    | CsShortcutBinding::HoldCtrlCmd
            )
        {
            return Err(CsError::Config {
                msg: "Assistive hold uses the dictation hold plus Shift; a second hold binding is released"
                    .to_string(),
            });
        }
        let mut settings = UserSettings::load();
        settings.set_mode_binding(mode.into(), binding.into());
        reload_hotkey_runtime_after_write();
        Ok(())
    }

    /// Clear all custom bindings back to the built-in defaults (Dictation=Hold Fn,
    /// Formatting=Double Left Option, Assistive=Double Right Option) and reload.
    pub fn reset_bindings_to_defaults(&self) -> Result<(), CsError> {
        let mut settings = UserSettings::load();
        // `None` normalizes to `default_mode_bindings()` on the next read, so this
        // is the canonical "reset" without hardcoding the default list twice.
        settings.mode_bindings = None;
        settings.save().map_err(|error| CsError::Config {
            msg: error.to_string(),
        })?;
        reload_hotkey_runtime_after_write();
        Ok(())
    }

    /// Validate a candidate binding set WITHOUT persisting it. Returns every
    /// detected conflict via the revived `shortcut_registry` (internal reachability
    /// collisions + macOS symbolic-hotkey collisions), plus the informational Fn
    /// tap-intercept note when relevant. Callers gate "save" on zero `blocking`
    /// entries.
    pub fn validate_bindings(&self, candidate: Vec<CsModeBinding>) -> Vec<CsHotkeyConflict> {
        let mode_bindings: Vec<ModeBinding> = candidate
            .iter()
            .map(|entry| ModeBinding {
                mode: entry.mode.into(),
                binding: entry.binding.into(),
            })
            .collect();
        let settings = UserSettings {
            mode_bindings: Some(mode_bindings),
            ..Default::default()
        };

        let mut conflicts: Vec<CsHotkeyConflict> = detect_hotkey_conflicts(&settings)
            .into_iter()
            .map(|conflict| CsHotkeyConflict {
                gesture_label: conflict.gesture.label().to_string(),
                message: conflict.message,
                blocking: true,
            })
            .collect();

        if let Some(note) = fn_tap_intercept_note(&settings) {
            conflicts.push(CsHotkeyConflict {
                gesture_label: "Hold Fn/Globe".to_string(),
                message: note.to_string(),
                blocking: false,
            });
        }

        conflicts
    }
}

/// The Settings mode-binding surface: FFI enum roundtrips, the re-arm gate,
/// conflict validation, and a persist/read-back cycle against an isolated
/// `CODESCRIBE_DATA_DIR`.
#[cfg(test)]
mod mode_binding_tests {
    use super::*;
    use serial_test::serial;
    use std::process::Command;
    use std::sync::Mutex;

    /// Serializes `CODESCRIBE_DATA_DIR` mutation for the persist/read-back test.
    // Serializes the CODESCRIBE_DATA_DIR-mutating test below within this module.
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    /// RED contract for the production-log pollution observed from this test
    /// module. A child test process gives `init_logging` a fresh `Once`, a fake
    /// HOME, and a distinct test data root; initialization must not create the
    /// production-shaped `~/.codescribe/logs/codescribe.log` sink.
    #[test]
    #[serial]
    fn fleet_red_test_logging_isolated() {
        const CHILD_ENV: &str = "CODESCRIBE_FLEET_RED_LOG_CHILD";
        if std::env::var_os(CHILD_ENV).is_some() {
            let production_path = std::path::PathBuf::from(
                std::env::var_os("HOME").expect("isolated child HOME must be set"),
            )
            .join(".codescribe")
            .join("logs")
            .join("codescribe.log");

            codescribe::logging::init_logging();
            assert!(
                !production_path.exists(),
                "test logger initialization resolved to production path: {}",
                production_path.display()
            );
            return;
        }

        let fake_home = tempfile::tempdir().expect("create isolated HOME");
        let test_data = tempfile::tempdir().expect("create isolated test data root");
        let status = Command::new(std::env::current_exe().expect("resolve test binary"))
            .args([
                "--exact",
                "hotkeys::mode_binding_tests::fleet_red_test_logging_isolated",
                "--nocapture",
            ])
            .env(CHILD_ENV, "1")
            .env("HOME", fake_home.path())
            .env("CODESCRIBE_DATA_DIR", test_data.path())
            .status()
            .expect("launch isolated logging child");

        assert!(
            status.success(),
            "isolated logger child must avoid the production log path"
        );
    }

    /// Every core work mode survives a UniFFI round-trip without loss.
    #[test]
    fn work_mode_ffi_round_trips() {
        for mode in ALL_WORK_MODES {
            let cs: CsWorkMode = mode.into();
            assert_eq!(WorkMode::from(cs), mode);
        }
    }

    /// Re-arm rebuilds only when the tap is down and both TCC gates are granted.
    #[test]
    fn rearm_gate_rebuilds_only_when_inactive_and_fully_granted() {
        use PermissionStatus::{Denied, Granted, NotDetermined};

        // The one case that arms: tap not yet live, both gating perms granted.
        assert!(should_rearm_hotkey_tap(false, Granted, Granted));

        // Dedup: an already-live tap is never torn down, even fully granted.
        assert!(!should_rearm_hotkey_tap(true, Granted, Granted));

        // Missing either gating permission must not trigger a doomed rebuild.
        assert!(!should_rearm_hotkey_tap(false, Denied, Granted));
        assert!(!should_rearm_hotkey_tap(false, Granted, Denied));
        assert!(!should_rearm_hotkey_tap(
            false,
            NotDetermined,
            NotDetermined
        ));

        // Already active + missing perms is still a no-op (both guards agree).
        assert!(!should_rearm_hotkey_tap(true, Denied, Denied));
    }

    /// Every closed-set gesture survives a UniFFI round-trip without loss.
    #[test]
    fn shortcut_binding_ffi_round_trips() {
        for binding in ALL_SHORTCUT_BINDINGS {
            let cs: CsShortcutBinding = binding.into();
            assert_eq!(ShortcutBinding::from(cs), binding);
        }
    }

    /// Settings picker options match `ALL_SHORTCUT_BINDINGS` length and labels.
    #[test]
    fn available_bindings_cover_the_closed_set() {
        let options = CodescribeHotkeys::new().available_bindings();
        assert_eq!(options.len(), ALL_SHORTCUT_BINDINGS.len());
        for (option, expected) in options.iter().zip(ALL_SHORTCUT_BINDINGS.iter()) {
            assert_eq!(ShortcutBinding::from(option.binding), *expected);
            assert!(!option.label.is_empty());
        }
    }

    /// Build a three-mode candidate binding set for `validate_bindings` tests.
    fn candidate(
        dictation: CsShortcutBinding,
        formatting: CsShortcutBinding,
        assistive: CsShortcutBinding,
    ) -> Vec<CsModeBinding> {
        vec![
            build_mode_binding(WorkMode::Dictation, dictation.into()),
            build_mode_binding(WorkMode::Formatting, formatting.into()),
            build_mode_binding(WorkMode::Assistive, assistive.into()),
        ]
    }

    /// Reachability collisions (e.g. DoubleCtrl vs DoubleLeftOption) are blocking.
    #[test]
    fn validate_flags_internal_reachability_conflict_as_blocking() {
        // Dictation=DoubleCtrl steals the toggle path from Formatting=DoubleLeftOption.
        let conflicts = CodescribeHotkeys::new().validate_bindings(candidate(
            CsShortcutBinding::DoubleCtrl,
            CsShortcutBinding::DoubleLeftOption,
            CsShortcutBinding::Disabled,
        ));
        assert!(
            conflicts.iter().any(|c| c.blocking),
            "a known reachability collision must surface a blocking conflict"
        );
    }

    /// A hold-only profile with disabled toggles validates with zero conflicts.
    #[test]
    fn validate_is_clean_for_a_safe_hold_only_profile() {
        // HoldCtrl never collides with macOS symbolic hotkeys and Disabled toggles
        // add nothing — a deterministic zero-conflict candidate on any machine.
        let conflicts = CodescribeHotkeys::new().validate_bindings(candidate(
            CsShortcutBinding::HoldCtrl,
            CsShortcutBinding::Disabled,
            CsShortcutBinding::Disabled,
        ));
        assert!(
            conflicts.is_empty(),
            "safe hold-only profile must validate clean, got {conflicts:?}"
        );
    }

    /// Persist/read-back cycle against an isolated `CODESCRIBE_DATA_DIR`.
    #[test]
    #[serial]
    fn set_mode_binding_persists_and_reads_back_through_the_bridge() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());

        let dir = std::env::temp_dir().join(format!("cs_bridge_hotkeys_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("create isolated data dir");
        let previous = std::env::var("CODESCRIBE_DATA_DIR").ok();
        // SAFETY: serialized by ENV_LOCK; env is restored before the lock drops.
        unsafe { std::env::set_var("CODESCRIBE_DATA_DIR", &dir) };

        let hotkeys = CodescribeHotkeys::new();
        hotkeys
            .set_mode_binding(CsWorkMode::Dictation, CsShortcutBinding::HoldCtrlAlt)
            .expect("set_mode_binding");

        let bindings = hotkeys.get_mode_bindings();
        let dictation = bindings
            .iter()
            .find(|b| b.mode == CsWorkMode::Dictation)
            .expect("dictation binding present");
        assert_eq!(dictation.binding, CsShortcutBinding::HoldCtrlAlt);

        // Reset restores defaults through the same path.
        hotkeys
            .reset_bindings_to_defaults()
            .expect("reset_bindings_to_defaults");
        let after_reset = hotkeys.get_mode_bindings();
        let dictation_reset = after_reset
            .iter()
            .find(|b| b.mode == CsWorkMode::Dictation)
            .expect("dictation binding present after reset");
        assert_eq!(dictation_reset.binding, CsShortcutBinding::HoldFn);

        // SAFETY: serialized by ENV_LOCK.
        unsafe {
            match previous {
                Some(value) => std::env::set_var("CODESCRIBE_DATA_DIR", value),
                None => std::env::remove_var("CODESCRIBE_DATA_DIR"),
            }
        }
        let _ = std::fs::remove_dir_all(&dir);
    }
}

/// Paste-result mapping across the FFI boundary: app names normalize to `None`
/// when blank, and a permission denial keeps both observed app names intact.
#[cfg(test)]
mod paste_target_mapping_tests {
    use super::{CsPasteOutcome, CsPasteResult, normalize_paste_target_app_name};
    use codescribe::controller::{OverlayPasteDelivery, OverlayPasteResult};

    /// Non-empty app names trim and stay present across the bridge mapping.
    #[test]
    fn bridge_mapping_keeps_present_app_name() {
        assert_eq!(
            normalize_paste_target_app_name(Some("  Ghostty  ".to_string())).as_deref(),
            Some("Ghostty")
        );
    }

    /// None / whitespace-only names collapse to absent for Swift labels.
    #[test]
    fn bridge_mapping_returns_absent_for_unknown_or_empty_name() {
        assert_eq!(normalize_paste_target_app_name(None), None);
        assert_eq!(
            normalize_paste_target_app_name(Some("   ".to_string())),
            None
        );
    }

    /// Accessibility denial keeps observed app names intact on the FFI result.
    #[test]
    fn accessibility_denial_maps_with_delivery_app_truth() {
        let result = CsPasteResult::from(OverlayPasteResult {
            delivery: OverlayPasteDelivery::AccessibilityPermissionNeeded,
            target_app_name: Some("Pensieve".to_string()),
            frontmost_app_name: Some("Pensieve".to_string()),
            deferred_insert_shortcut: None,
            deferred_insert_failure: None,
        });

        assert_eq!(
            result.outcome,
            CsPasteOutcome::AccessibilityPermissionNeeded
        );
        assert_eq!(result.target_app_name.as_deref(), Some("Pensieve"));
        assert_eq!(result.frontmost_app_name.as_deref(), Some("Pensieve"));
    }
}

// ===========================================================================
// Orphaned optimistic-overlay compensation (CUT P0a)
//
// Contract under test: any dispatch that shows the optimistic "preparing"
// overlay is guaranteed a terminal listener event. When the controller ends the
// dispatch back at Idle WITHOUT a StateChange broadcast — the shape produced by
// the quick hold-release cancel (`cancel_pending_hold_start`), the start-failure
// reset (`reset_session_after_start_failure` → `set_state(Idle)` at old==Idle),
// and the no-op re-check dispatch — `compensate_orphaned_preparing` emits exactly
// one compensating `on_recording_stopped`. When a real transition occurred the
// broadcast forwarder owns the terminal event and the compensator must NOT
// double-fire.
// ===========================================================================
/// The terminal-event guarantee for the optimistic overlay described in the
/// banner above, plus the `busy` → finalising → `idle` → stopped forwarding
/// sequence a real hold-release produces.
#[cfg(test)]
mod preparing_compensation_tests {
    use super::*;
    use serial_test::serial;
    use std::sync::Mutex as StdMutex;
    use std::sync::atomic::AtomicUsize;
    use tokio::sync::Mutex as AsyncMutex;

    /// Async mutex serializing process-global PREPARING/listener/controller state.
    // Serializes the process-global PREPARING_PENDING / shared_listener /
    // shared_controller these tests mutate, so parallel runs don't interleave.
    // Async-aware so the guard can be held across the `.await` points below.
    static TEST_LOCK: AsyncMutex<()> = AsyncMutex::const_new(());

    /// Capturing transcription listener for optimistic-overlay compensation tests.
    #[derive(Default)]
    struct RecordingLifecycleListener {
        preparing: AtomicUsize,
        started: AtomicUsize,
        stopped: AtomicUsize,
        finalising: AtomicUsize,
        audio_levels: StdMutex<Vec<f32>>,
        context_markers: StdMutex<Vec<(u64, String)>>,
    }

    impl RecordingLifecycleListener {
        /// Count of `on_recording_preparing` callbacks observed.
        fn preparing(&self) -> usize {
            self.preparing.load(Ordering::SeqCst)
        }
        /// Count of `on_recording_started` callbacks observed.
        fn started(&self) -> usize {
            self.started.load(Ordering::SeqCst)
        }
        /// Count of `on_recording_stopped` callbacks observed.
        fn stopped(&self) -> usize {
            self.stopped.load(Ordering::SeqCst)
        }
        /// Count of `on_recording_finalising` callbacks observed.
        fn finalising(&self) -> usize {
            self.finalising.load(Ordering::SeqCst)
        }
        /// Snapshot of RMS audio-level samples the listener captured.
        fn audio_levels(&self) -> Vec<f32> {
            self.audio_levels
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .clone()
        }
        /// Snapshot of context markers `(position, label)` the listener captured.
        fn context_markers(&self) -> Vec<(u64, String)> {
            self.context_markers
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .clone()
        }
    }

    impl CsTranscriptionListener for RecordingLifecycleListener {
        /// Count preparing overlay shows for compensation assertions.
        fn on_recording_preparing(&self) {
            self.preparing.fetch_add(1, Ordering::SeqCst);
        }
        /// Count real start transitions from the broadcast forwarder.
        fn on_recording_started(&self) {
            self.started.fetch_add(1, Ordering::SeqCst);
        }
        /// Count terminal stop events (broadcast or compensating).
        fn on_recording_stopped(&self) {
            self.stopped.fetch_add(1, Ordering::SeqCst);
        }
        /// Count busy→finalising transitions for the hold-release sequence.
        fn on_recording_finalising(&self) {
            self.finalising.fetch_add(1, Ordering::SeqCst);
        }
        /// No-op: preview text is not under test in this suite.
        fn on_preview(&self, _text: String) {}
        /// No-op: mid-stream corrections are not under test in this suite.
        fn on_correction(&self, _text: String, _previous_text: String) {}
        /// No-op: utterance finals are not under test in this suite.
        fn on_final(
            &self,
            _utterance_id: u64,
            _text: String,
            _avg_logprob: Option<f32>,
            _speech_pct: Option<f32>,
            _confidence_flags: Vec<String>,
        ) {
        }
        /// No-op: layered replace-range patches are not under test here.
        fn on_replace_range(
            &self,
            _utterance_id: u64,
            _start: u64,
            _end: u64,
            _text: String,
            _source: crate::recording::CsLayerSource,
        ) {
        }
        /// No-op: annotation inserts are not under test in this suite.
        fn on_insert_annotation(
            &self,
            _utterance_id: u64,
            _position: u64,
            _text: String,
            _kind: CsAnnotationKind,
        ) {
        }
        /// Capture selection/context markers for forwarder assertions.
        fn on_context_marker(&self, position: u64, marker: String) {
            self.context_markers
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .push((position, marker));
        }
        /// No-op: session finalisation summary is not under test here.
        fn on_session_finalised(&self, _session_id: String, _layer_summary: CsLayerSummary) {}
        /// No-op: final transcript ready is not under test in this suite.
        fn on_final_transcript_ready(&self, _text: String) {}
        /// No-op: VAD active toggles are not under test in this suite.
        fn on_vad_active(&self, _active: bool) {}
        /// Capture RMS samples so audio-level forwarding can be asserted.
        fn on_audio_level(&self, rms: f32) {
            self.audio_levels
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .push(rms);
        }
        /// No-op: no-speech reasons are not under test in this suite.
        fn on_no_speech(&self, _reason: String) {}
        /// No-op: error messages are not under test in this suite.
        fn on_error(&self, _message: String) {}
    }

    /// Install a fresh capturing listener + an Idle controller into the shared
    /// process stores and clear the pending flag. Returns both so the test can
    /// assert on the listener and pass the controller to the compensator.
    fn install() -> (Arc<RecordingLifecycleListener>, Arc<RecordingController>) {
        PREPARING_PENDING.store(false, Ordering::SeqCst);
        let listener = Arc::new(RecordingLifecycleListener::default());
        *shared_listener().write().unwrap_or_else(|e| e.into_inner()) =
            Some(Arc::clone(&listener) as Arc<dyn CsTranscriptionListener>);
        let controller = Arc::new(RecordingController::new_without_keychain());
        *shared_controller()
            .lock()
            .unwrap_or_else(|e| e.into_inner()) = Some(Arc::clone(&controller));
        (listener, controller)
    }

    /// Clear shared listener/controller stores and the preparing flag after a test.
    fn teardown() {
        *shared_listener().write().unwrap_or_else(|e| e.into_inner()) = None;
        *shared_controller()
            .lock()
            .unwrap_or_else(|e| e.into_inner()) = None;
        PREPARING_PENDING.store(false, Ordering::SeqCst);
    }

    /// A host lifecycle notification while idle must not construct the shared
    /// controller (and therefore cannot prewarm or load an engine).
    #[tokio::test]
    #[serial]
    async fn sleep_wake_without_active_controller_is_a_noop() {
        let _guard = TEST_LOCK.lock().await;
        teardown();

        let hotkeys = CodescribeHotkeys::new();
        assert!(!hotkeys.note_sleep_wake().await);
        assert!(current_controller(&shared_controller()).is_none());
    }

    /// AudioLevel IPC payload forwards the RMS sample to the Swift listener.
    #[test]
    fn recording_audio_level_payload_forwards_rms() {
        let listener = Arc::new(RecordingLifecycleListener::default());
        forward_event_to_listener(
            IpcEventPayload::AudioLevel { rms: 0.125 },
            Arc::clone(&listener) as Arc<dyn CsTranscriptionListener>,
        );
        assert_eq!(listener.audio_levels(), vec![0.125]);
    }

    /// ContextMarker IPC payload forwards position + label to the listener.
    #[test]
    fn context_marker_payload_forwards_position_and_label() {
        let listener = Arc::new(RecordingLifecycleListener::default());
        forward_event_to_listener(
            IpcEventPayload::ContextMarker {
                position: 7,
                marker: "{selection_3}".to_string(),
            },
            Arc::clone(&listener) as Arc<dyn CsTranscriptionListener>,
        );
        assert_eq!(
            listener.context_markers(),
            vec![(7, "{selection_3}".to_string())]
        );
    }

    /// Paths 1 & 2 (quick hold-release cancel, start-failure reset): preparing was
    /// shown, the controller ended the dispatch at Idle with no broadcast → the
    /// compensator must emit exactly one terminal stop.
    #[tokio::test]
    #[serial]
    async fn orphaned_preparing_at_idle_gets_a_compensating_stop() {
        let _guard = TEST_LOCK.lock().await;
        let (listener, controller) = install();

        // The optimistic overlay is shown for a start gesture at Idle.
        optimistically_show_overlay(&HotkeyEvent::ToggleNormal).await;
        assert_eq!(listener.preparing(), 1, "preparing overlay must be shown");
        assert!(PREPARING_PENDING.load(Ordering::SeqCst), "flag armed");

        // The dispatch left the controller at Idle without any StateChange
        // (the shape of cancel_pending_hold_start / start-failure reset).
        compensate_orphaned_preparing(&controller).await;

        assert_eq!(
            listener.stopped(),
            1,
            "orphaned preparing must receive one terminal stop"
        );
        assert!(!PREPARING_PENDING.load(Ordering::SeqCst), "flag cleared");
        teardown();
    }

    /// The compensator is inert when no optimistic overlay was shown: an ordinary
    /// stop dispatch (controller back at Idle, but flag never armed) must not have a
    /// spurious extra stop synthesized on top of the broadcast one.
    #[tokio::test]
    #[serial]
    async fn no_preparing_shown_means_no_compensating_stop() {
        let _guard = TEST_LOCK.lock().await;
        let (listener, controller) = install();

        compensate_orphaned_preparing(&controller).await;

        assert_eq!(listener.preparing(), 0);
        assert_eq!(
            listener.stopped(),
            0,
            "no preparing was pending, so nothing to compensate"
        );
        teardown();
    }

    /// Idempotency: a second compensator pass (e.g. the FFI `start_recording` path
    /// racing the hotkey spawn) must not emit a second stop for the same overlay.
    #[tokio::test]
    #[serial]
    async fn compensation_is_idempotent_across_repeated_passes() {
        let _guard = TEST_LOCK.lock().await;
        let (listener, controller) = install();

        optimistically_show_overlay(&HotkeyEvent::ToggleNormal).await;
        compensate_orphaned_preparing(&controller).await;
        compensate_orphaned_preparing(&controller).await;

        assert_eq!(
            listener.stopped(),
            1,
            "the compensating stop must fire at most once per preparing"
        );
        teardown();
    }

    /// Path 3 (no-op dispatch) / genuine start: when a real transition's broadcast
    /// already resolved the preparing (forwarder cleared the flag and emitted
    /// started), the compensator must not double-fire a stop on top of it.
    #[tokio::test]
    #[serial]
    async fn forwarder_resolved_preparing_is_not_double_stopped() {
        let _guard = TEST_LOCK.lock().await;
        let (listener, controller) = install();

        optimistically_show_overlay(&HotkeyEvent::ToggleNormal).await;
        assert!(PREPARING_PENDING.load(Ordering::SeqCst));

        // Simulate the broadcast forwarder observing a real Idle→rec_toggle
        // transition: it emits started and clears the pending flag.
        forward_event_to_listener(
            IpcEventPayload::StateChange {
                from: "idle".to_string(),
                to: "rec_toggle".to_string(),
            },
            Arc::clone(&listener) as Arc<dyn CsTranscriptionListener>,
        );
        assert_eq!(listener.started(), 1, "forwarder emitted started");
        assert!(
            !PREPARING_PENDING.load(Ordering::SeqCst),
            "forwarder cleared flag"
        );

        // A late compensator pass (controller now back at Idle) must stay silent —
        // the started already resolved the overlay.
        compensate_orphaned_preparing(&controller).await;
        assert_eq!(
            listener.stopped(),
            0,
            "a forwarder-resolved preparing must not be double-stopped"
        );
        teardown();
    }

    /// The `Busy` StateChange (final transcription pass, after capture ends) routes
    /// to `on_recording_finalising` — the native-path signal that lets the overlay
    /// enter its "transcribing" phase — and NOT to started/stopped. The terminal
    /// `idle` still maps to `on_recording_stopped`, so the sequence a real
    /// hold-release / toggle stop produces (rec_hold → busy → idle) yields exactly
    /// one finalising then one stopped.
    #[tokio::test]
    #[serial]
    async fn busy_state_routes_to_finalising_then_idle_to_stopped() {
        let _guard = TEST_LOCK.lock().await;
        let (listener, _controller) = install();
        let dyn_listener = || Arc::clone(&listener) as Arc<dyn CsTranscriptionListener>;

        forward_event_to_listener(
            IpcEventPayload::StateChange {
                from: "rec_hold".to_string(),
                to: "busy".to_string(),
            },
            dyn_listener(),
        );
        assert_eq!(listener.finalising(), 1, "busy → finalising");
        assert_eq!(listener.stopped(), 0, "busy must not fire stopped");
        assert_eq!(listener.started(), 0, "busy must not fire started");

        forward_event_to_listener(
            IpcEventPayload::StateChange {
                from: "busy".to_string(),
                to: "idle".to_string(),
            },
            dyn_listener(),
        );
        assert_eq!(listener.stopped(), 1, "idle → stopped");
        assert_eq!(listener.finalising(), 1, "idle must not re-fire finalising");
        teardown();
    }

    /// A repeated `Busy` broadcast forwards a second `on_recording_finalising`; the
    /// idempotency that matters (a no-op re-entry) lives in the Swift handler, so
    /// the forwarder stays a thin, stateless router here.
    #[tokio::test]
    #[serial]
    async fn repeated_busy_forwards_each_finalising() {
        let _guard = TEST_LOCK.lock().await;
        let (listener, _controller) = install();
        let dyn_listener = || Arc::clone(&listener) as Arc<dyn CsTranscriptionListener>;

        for _ in 0..2 {
            forward_event_to_listener(
                IpcEventPayload::StateChange {
                    from: "rec_hold".to_string(),
                    to: "busy".to_string(),
                },
                dyn_listener(),
            );
        }
        assert_eq!(listener.finalising(), 2, "each busy forwards a finalising");
        assert!(
            !PREPARING_PENDING.load(Ordering::SeqCst),
            "busy must not arm the preparing flag"
        );
        teardown();
    }
}
