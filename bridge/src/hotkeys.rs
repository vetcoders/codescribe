//! Global hotkey runtime surface for the SwiftUI redesign.
//!
//! This does not reimplement hotkeys in Swift. It starts the same macOS
//! `CGEventTap` listener used by the legacy daemon and dispatches emitted
//! `HotkeyEvent`s into the existing `RecordingController` state machine.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock, RwLock};

use codescribe::controller::{HotkeyAction, HotkeyInput, HotkeyType, RecordingController, State};
use codescribe::os::hotkeys::{self, HoldAction, HoldMode, HotkeyEvent};
use codescribe::os::permissions::{PermissionStatus, check_accessibility, check_input_monitoring};
use codescribe::os::shortcut_registry::{detect_hotkey_conflicts, fn_tap_intercept_note};
use codescribe::os::tray_status::{self, TrayStatus};
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
use crate::recording::{CsAnnotationKind, CsLayerSummary, CsTranscriptionListener};
use crate::{CsError, CsLanguage};

type SharedController = Arc<Mutex<Option<Arc<RecordingController>>>>;
type SharedListener = Arc<RwLock<Option<Arc<dyn CsTranscriptionListener>>>>;
type SharedAppActionListener = Arc<RwLock<Option<Arc<dyn CsAppActionListener>>>>;

/// Foreign callback for UI-only global commands. These actions are deliberately
/// separate from `CsTranscriptionListener`: they carry no audio or model payload
/// and must never enter the recording controller path.
#[uniffi::export(with_foreign)]
pub trait CsAppActionListener: Send + Sync {
    fn on_show_agent(&self);
}

fn shared_controller() -> SharedController {
    static CONTROLLER: OnceLock<SharedController> = OnceLock::new();
    Arc::clone(CONTROLLER.get_or_init(|| Arc::new(Mutex::new(None))))
}

fn shared_listener() -> SharedListener {
    static LISTENER: OnceLock<SharedListener> = OnceLock::new();
    Arc::clone(LISTENER.get_or_init(|| Arc::new(RwLock::new(None))))
}

fn shared_app_action_listener() -> SharedAppActionListener {
    static LISTENER: OnceLock<SharedAppActionListener> = OnceLock::new();
    Arc::clone(LISTENER.get_or_init(|| Arc::new(RwLock::new(None))))
}

fn current_app_action_listener() -> Option<Arc<dyn CsAppActionListener>> {
    shared_app_action_listener()
        .read()
        .unwrap_or_else(|e| e.into_inner())
        .as_ref()
        .map(Arc::clone)
}

fn route_hotkey_event<F>(
    event: HotkeyEvent,
    app_action_listener: Option<Arc<dyn CsAppActionListener>>,
    dispatch_recording: F,
) where
    F: FnOnce(HotkeyEvent),
{
    match event {
        HotkeyEvent::ShowAgent => {
            tracing::info!("Agent summon command: dispatching UI-only app action");
            if let Some(listener) = app_action_listener {
                listener.on_show_agent();
            }
        }
        recording_event => dispatch_recording(recording_event),
    }
}

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

fn current_controller(controller_store: &SharedController) -> Option<Arc<RecordingController>> {
    controller_store
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .as_ref()
        .map(Arc::clone)
}

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
        IpcEventPayload::AudioLevel { rms } => listener.on_audio_level(rms),
        IpcEventPayload::Engine(event) => match event {
            EngineEventWire::VadStart { .. } => listener.on_vad_active(true),
            EngineEventWire::VadEnd { .. } => listener.on_vad_active(false),
            EngineEventWire::NoSpeech { reason } => listener.on_no_speech(reason),
            EngineEventWire::Preview { text, .. } => listener.on_preview(text),
            EngineEventWire::Correction {
                text,
                previous_text,
                ..
            } => listener.on_correction(text, previous_text),
            EngineEventWire::UtteranceFinal {
                utterance_id, text, ..
            } => listener.on_final(utterance_id, text),
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
            EngineEventWire::Warning { code, message } => {
                tray_status::update_tray_status(TrayStatus::Error);
                listener.on_error(format!("{code}: {message}"));
            }
            EngineEventWire::Drop { .. } | EngineEventWire::Stats { .. } => {}
        },
    }
}

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

#[uniffi::export(async_runtime = "tokio")]
impl CodescribeHotkeys {
    #[uniffi::constructor]
    pub fn new() -> Self {
        codescribe::logging::init_logging();
        Self::default()
    }

    /// Start or replace the process-global hotkey listener.
    pub async fn start(&self) -> Result<(), CsError> {
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

        // Spawn the event-dispatch thread BEFORE bringing up the tap. It drains
        // `rx` for the lifetime of the retained sender, so it stays ready whether
        // the CGEventTap comes up now (permissions already granted) or later via
        // `rearm_after_permission_grant` after a first-run TCC grant. If it were
        // spawned only after a successful `install_global_hotkey_manager`, a
        // permission-less cold start would leave no consumer, and a later re-arm
        // would build a live tap whose events pile up in the channel undispatched.
        std::thread::spawn(move || {
            for event in rx {
                let spawn_handle = handle.clone();
                let controller_handle = handle.clone();
                let controller_store = Arc::clone(&controller_store);
                route_hotkey_event(
                    event,
                    current_app_action_listener(),
                    move |recording_event| {
                        spawn_handle.spawn(async move {
                            optimistically_show_overlay(&recording_event).await;
                            let controller =
                                ensure_controller(&controller_store, controller_handle);
                            let dispatch = dispatch_recording_hotkey_event(
                                recording_event,
                                Arc::clone(&controller),
                            )
                            .await;
                            compensate_orphaned_preparing(&controller).await;
                            if let Err(error) = dispatch {
                                tray_status::update_tray_status(TrayStatus::Error);
                                eprintln!("Hotkey event error: {error}");
                            }
                        });
                    },
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
    }

    /// Start the same toggle recording flow used by the default hotkey.
    pub async fn start_recording(&self) -> Result<(), CsError> {
        start_recording_with_event(HotkeyEvent::ToggleNormal).await
    }

    /// Start the same toggle flow in the assistive lane for UI-initiated recording.
    pub async fn start_assistive_recording(&self) -> Result<(), CsError> {
        start_recording_with_event(HotkeyEvent::ToggleAssistive).await
    }

    /// Stop the active legacy-controller recording flow, if one is live.
    pub async fn stop_recording(&self) -> Result<(), CsError> {
        let Some(controller) = current_controller(&shared_controller()) else {
            return Ok(());
        };
        controller
            .stop_recording_from_external_surface()
            .await
            .map_err(|error| CsError::Recording {
                msg: error.to_string(),
            })
    }

    /// True while the shared controller is in an active recording/conversation state.
    pub async fn is_recording(&self) -> bool {
        let Some(controller) = current_controller(&shared_controller()) else {
            return false;
        };
        matches!(
            controller.current_state().await,
            codescribe::controller::State::RecHold
                | codescribe::controller::State::RecToggle
                | codescribe::controller::State::Conversation
        )
    }

    /// True when the configured formatting provider can handle a user-triggered
    /// overlay format action.
    pub fn is_formatting_available(&self) -> bool {
        codescribe::ai_formatting::is_formatting_available()
    }

    /// Format editable overlay text after recording stops.
    pub async fn format_text(
        &self,
        text: String,
        language: Option<CsLanguage>,
    ) -> Result<String, CsError> {
        let language = language.map(|l| l.as_code().to_string());
        let result = codescribe::ai_formatting::format_text_with_status(
            &text,
            language.as_deref(),
            false,
            None,
        )
        .await;
        if result.text.trim().is_empty() {
            Ok(text)
        } else {
            Ok(result.text)
        }
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
        )
        .await;
        if result.text.trim().is_empty() {
            Ok(text)
        } else {
            Ok(result.text)
        }
    }

    /// Paste edited overlay text back into the app that was frontmost before the
    /// overlay. Returns the honest delivery outcome: `Pasted`, or
    /// `CopiedToClipboard` when the controller's self-paste guard degraded the
    /// action to a tagged clipboard copy.
    pub async fn paste_text(&self, text: String) -> Result<CsPasteOutcome, CsError> {
        let controller = ensure_controller(&shared_controller(), tokio::runtime::Handle::current());
        controller
            .paste_text_from_overlay(text)
            .await
            .map(CsPasteOutcome::from)
            .map_err(|error| CsError::Recording {
                msg: error.to_string(),
            })
    }

    /// Copy the tagged transcript to the clipboard without a synthetic paste.
    /// Swift calls this when the caret already sits inside Codescribe, where a
    /// synthetic Cmd+V would paste the transcript back into the overlay itself.
    pub async fn copy_text_tagged(&self, text: String) -> Result<(), CsError> {
        let controller = ensure_controller(&shared_controller(), tokio::runtime::Handle::current());
        controller
            .copy_text_from_overlay(text)
            .await
            .map_err(|error| CsError::Recording {
                msg: error.to_string(),
            })
    }

    /// Name of the app latched for the current overlay session, if known.
    /// Read-only: the paste path keeps owning target activation and delivery.
    pub async fn paste_target_app_name(&self) -> Option<String> {
        let controller = current_controller(&shared_controller())?;
        normalize_paste_target_app_name(controller.paste_target_app_name().await)
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
}

/// Honest outcome of the overlay Insert action, mirrored to Swift so the UI
/// can tell the user when the self-paste guard degraded a paste to a tagged
/// clipboard copy.
#[derive(uniffi::Enum, Debug, Clone, Copy, PartialEq, Eq)]
pub enum CsPasteOutcome {
    Pasted,
    CopiedToClipboard,
    Noop,
}

impl From<codescribe::controller::OverlayPasteDelivery> for CsPasteOutcome {
    fn from(value: codescribe::controller::OverlayPasteDelivery) -> Self {
        match value {
            codescribe::controller::OverlayPasteDelivery::Pasted => Self::Pasted,
            codescribe::controller::OverlayPasteDelivery::CopiedToClipboard => {
                Self::CopiedToClipboard
            }
            codescribe::controller::OverlayPasteDelivery::Noop => Self::Noop,
        }
    }
}

async fn start_recording_with_event(event: HotkeyEvent) -> Result<(), CsError> {
    optimistically_show_overlay(&event).await;
    let controller = ensure_controller(&shared_controller(), tokio::runtime::Handle::current());
    let dispatch = dispatch_recording_hotkey_event(event, Arc::clone(&controller)).await;
    compensate_orphaned_preparing(&controller).await;
    dispatch.map_err(|error| CsError::Recording {
        msg: error.to_string(),
    })
}

async fn dispatch_recording_hotkey_event(
    event: HotkeyEvent,
    controller: Arc<RecordingController>,
) -> anyhow::Result<()> {
    match event {
        HotkeyEvent::ShowAgent => {
            unreachable!("ShowAgent must be routed before recording dispatch")
        }
        HotkeyEvent::Hold {
            action,
            mode,
            force_ai,
        } => {
            let mapped_action = match action {
                HoldAction::Down => HotkeyAction::Down,
                HoldAction::Up => HotkeyAction::Up,
            };
            let input = HotkeyInput {
                key_type: HotkeyType::Hold,
                action: mapped_action,
                assistive: !matches!(mode, HoldMode::Raw),
                hold_mode: mode,
                force_raw: matches!(mode, HoldMode::Raw) && !force_ai,
                force_ai,
            };
            controller.handle_hotkey_event(input).await?;
        }
        HotkeyEvent::HoldUpdate { mode, force_ai } => {
            let input = HotkeyInput {
                key_type: HotkeyType::Hold,
                action: HotkeyAction::Press,
                assistive: !matches!(mode, HoldMode::Raw),
                hold_mode: mode,
                force_raw: matches!(mode, HoldMode::Raw) && !force_ai,
                force_ai,
            };
            controller.handle_hotkey_event(input).await?;
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
            let body = format!(
                "{} was detected, but {}.",
                gesture.label(),
                reason.message()
            );
            eprintln!("Hotkey double-tap blocked: {body}");
        }
    }

    Ok(())
}

#[cfg(test)]
mod format_level_tests {
    use super::*;

    #[tokio::test]
    async fn format_text_for_level_rejects_unknown_level() {
        let hotkeys = CodescribeHotkeys::default();
        let result = hotkeys
            .format_text_for_level("hello".to_string(), None, "mega".to_string())
            .await;
        assert!(matches!(result, Err(CsError::Config { .. })));
    }

    #[tokio::test]
    async fn format_text_for_level_rejects_off() {
        let hotkeys = CodescribeHotkeys::default();
        let result = hotkeys
            .format_text_for_level("hello".to_string(), None, "off".to_string())
            .await;
        assert!(matches!(result, Err(CsError::Config { .. })));
    }

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

#[cfg(test)]
mod dispatch_tests {
    use super::*;
    use codescribe::os::hotkeys::{DoubleTapBlockReason, DoubleTapGesture};

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

#[cfg(test)]
mod app_action_tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;

    struct CountingAppActionListener {
        show_agent_calls: AtomicUsize,
    }

    impl CsAppActionListener for CountingAppActionListener {
        fn on_show_agent(&self) {
            self.show_agent_calls.fetch_add(1, Ordering::SeqCst);
        }
    }

    #[test]
    fn show_agent_routes_without_recording_or_preparing_payload() {
        PREPARING_PENDING.store(false, Ordering::SeqCst);
        let listener = Arc::new(CountingAppActionListener {
            show_agent_calls: AtomicUsize::new(0),
        });
        let recording_calls = Arc::new(AtomicUsize::new(0));
        let recording_calls_for_route = Arc::clone(&recording_calls);

        route_hotkey_event(HotkeyEvent::ShowAgent, Some(listener.clone()), move |_| {
            recording_calls_for_route.fetch_add(1, Ordering::SeqCst);
        });

        assert_eq!(listener.show_agent_calls.load(Ordering::SeqCst), 1);
        assert_eq!(recording_calls.load(Ordering::SeqCst), 0);
        assert!(!PREPARING_PENDING.load(Ordering::SeqCst));

        let recording_calls_for_route = Arc::clone(&recording_calls);
        route_hotkey_event(
            HotkeyEvent::ToggleNormal,
            Some(listener.clone()),
            move |_| {
                recording_calls_for_route.fetch_add(1, Ordering::SeqCst);
            },
        );
        assert_eq!(listener.show_agent_calls.load(Ordering::SeqCst), 1);
        assert_eq!(recording_calls.load(Ordering::SeqCst), 1);
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
    fn from(mode: WorkMode) -> Self {
        match mode {
            WorkMode::Dictation => CsWorkMode::Dictation,
            WorkMode::Formatting => CsWorkMode::Formatting,
            WorkMode::Assistive => CsWorkMode::Assistive,
        }
    }
}

impl From<CsWorkMode> for WorkMode {
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

const ALL_WORK_MODES: [WorkMode; 3] = [
    WorkMode::Dictation,
    WorkMode::Formatting,
    WorkMode::Assistive,
];

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

#[cfg(test)]
mod mode_binding_tests {
    use super::*;
    use serial_test::serial;
    use std::sync::Mutex;

    // Serializes the CODESCRIBE_DATA_DIR-mutating test below within this module.
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn work_mode_ffi_round_trips() {
        for mode in ALL_WORK_MODES {
            let cs: CsWorkMode = mode.into();
            assert_eq!(WorkMode::from(cs), mode);
        }
    }

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

    #[test]
    fn shortcut_binding_ffi_round_trips() {
        for binding in ALL_SHORTCUT_BINDINGS {
            let cs: CsShortcutBinding = binding.into();
            assert_eq!(ShortcutBinding::from(cs), binding);
        }
    }

    #[test]
    fn available_bindings_cover_the_closed_set() {
        let options = CodescribeHotkeys::new().available_bindings();
        assert_eq!(options.len(), ALL_SHORTCUT_BINDINGS.len());
        for (option, expected) in options.iter().zip(ALL_SHORTCUT_BINDINGS.iter()) {
            assert_eq!(ShortcutBinding::from(option.binding), *expected);
            assert!(!option.label.is_empty());
        }
    }

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

#[cfg(test)]
mod paste_target_mapping_tests {
    use super::normalize_paste_target_app_name;

    #[test]
    fn bridge_mapping_keeps_present_app_name() {
        assert_eq!(
            normalize_paste_target_app_name(Some("  Ghostty  ".to_string())).as_deref(),
            Some("Ghostty")
        );
    }

    #[test]
    fn bridge_mapping_returns_absent_for_unknown_or_empty_name() {
        assert_eq!(normalize_paste_target_app_name(None), None);
        assert_eq!(
            normalize_paste_target_app_name(Some("   ".to_string())),
            None
        );
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
#[cfg(test)]
mod preparing_compensation_tests {
    use super::*;
    use serial_test::serial;
    use std::sync::Mutex as StdMutex;
    use std::sync::atomic::AtomicUsize;
    use tokio::sync::Mutex as AsyncMutex;

    // Serializes the process-global PREPARING_PENDING / shared_listener /
    // shared_controller these tests mutate, so parallel runs don't interleave.
    // Async-aware so the guard can be held across the `.await` points below.
    static TEST_LOCK: AsyncMutex<()> = AsyncMutex::const_new(());

    #[derive(Default)]
    struct RecordingLifecycleListener {
        preparing: AtomicUsize,
        started: AtomicUsize,
        stopped: AtomicUsize,
        finalising: AtomicUsize,
        audio_levels: StdMutex<Vec<f32>>,
    }

    impl RecordingLifecycleListener {
        fn preparing(&self) -> usize {
            self.preparing.load(Ordering::SeqCst)
        }
        fn started(&self) -> usize {
            self.started.load(Ordering::SeqCst)
        }
        fn stopped(&self) -> usize {
            self.stopped.load(Ordering::SeqCst)
        }
        fn finalising(&self) -> usize {
            self.finalising.load(Ordering::SeqCst)
        }
        fn audio_levels(&self) -> Vec<f32> {
            self.audio_levels
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .clone()
        }
    }

    impl CsTranscriptionListener for RecordingLifecycleListener {
        fn on_recording_preparing(&self) {
            self.preparing.fetch_add(1, Ordering::SeqCst);
        }
        fn on_recording_started(&self) {
            self.started.fetch_add(1, Ordering::SeqCst);
        }
        fn on_recording_stopped(&self) {
            self.stopped.fetch_add(1, Ordering::SeqCst);
        }
        fn on_recording_finalising(&self) {
            self.finalising.fetch_add(1, Ordering::SeqCst);
        }
        fn on_preview(&self, _text: String) {}
        fn on_correction(&self, _text: String, _previous_text: String) {}
        fn on_final(&self, _utterance_id: u64, _text: String) {}
        fn on_replace_range(
            &self,
            _utterance_id: u64,
            _start: u64,
            _end: u64,
            _text: String,
            _source: crate::recording::CsLayerSource,
        ) {
        }
        fn on_insert_annotation(
            &self,
            _utterance_id: u64,
            _position: u64,
            _text: String,
            _kind: CsAnnotationKind,
        ) {
        }
        fn on_session_finalised(&self, _session_id: String, _layer_summary: CsLayerSummary) {}
        fn on_final_transcript_ready(&self, _text: String) {}
        fn on_vad_active(&self, _active: bool) {}
        fn on_audio_level(&self, rms: f32) {
            self.audio_levels
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .push(rms);
        }
        fn on_no_speech(&self, _reason: String) {}
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

    fn teardown() {
        *shared_listener().write().unwrap_or_else(|e| e.into_inner()) = None;
        *shared_controller()
            .lock()
            .unwrap_or_else(|e| e.into_inner()) = None;
        PREPARING_PENDING.store(false, Ordering::SeqCst);
    }

    #[test]
    fn recording_audio_level_payload_forwards_rms() {
        let listener = Arc::new(RecordingLifecycleListener::default());
        forward_event_to_listener(
            IpcEventPayload::AudioLevel { rms: 0.125 },
            Arc::clone(&listener) as Arc<dyn CsTranscriptionListener>,
        );
        assert_eq!(listener.audio_levels(), vec![0.125]);
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
