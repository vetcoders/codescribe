//! Global hotkey runtime surface for the SwiftUI redesign.
//!
//! This does not reimplement hotkeys in Swift. It starts the same macOS
//! `CGEventTap` listener used by the legacy daemon and dispatches emitted
//! `HotkeyEvent`s into the existing `RecordingController` state machine.

use std::sync::{Arc, Mutex, OnceLock, RwLock};

use codescribe::controller::{HotkeyAction, HotkeyInput, HotkeyType, RecordingController, State};
use codescribe::os::hotkeys::{self, HoldAction, HoldMode, HotkeyEvent};
use codescribe_core::ipc::{EngineEventWire, IpcEventPayload};
use crossbeam_channel::unbounded;
use tokio::runtime::Handle;

use crate::recording::CsTranscriptionListener;
use crate::{CsError, CsLanguage};

type SharedController = Arc<Mutex<Option<Arc<RecordingController>>>>;
type SharedListener = Arc<RwLock<Option<Arc<dyn CsTranscriptionListener>>>>;

fn shared_controller() -> SharedController {
    static CONTROLLER: OnceLock<SharedController> = OnceLock::new();
    Arc::clone(CONTROLLER.get_or_init(|| Arc::new(Mutex::new(None))))
}

fn shared_listener() -> SharedListener {
    static LISTENER: OnceLock<SharedListener> = OnceLock::new();
    Arc::clone(LISTENER.get_or_init(|| Arc::new(RwLock::new(None))))
}

fn ensure_controller(
    controller_store: &SharedController,
    handle: Handle,
) -> Arc<RecordingController> {
    let mut guard = controller_store.lock().unwrap_or_else(|e| e.into_inner());
    Arc::clone(guard.get_or_insert_with(|| {
        let controller = Arc::new(RecordingController::new());
        codescribe::controller::register_overlay_controller(Arc::clone(&controller));
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

fn spawn_event_forwarder(controller: Arc<RecordingController>, handle: Handle) {
    let listener_store = shared_listener();
    handle.spawn(async move {
        let mut events = controller.subscribe_events();
        loop {
            let Ok(event) = events.recv().await else {
                break;
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
            "rec_hold" | "rec_toggle" | "conversation" => listener.on_recording_started(),
            "idle" => listener.on_recording_stopped(),
            _ => {}
        },
        IpcEventPayload::Engine(event) => match event {
            EngineEventWire::VadStart { .. } => listener.on_vad_active(true),
            EngineEventWire::VadEnd { .. } => listener.on_vad_active(false),
            EngineEventWire::NoSpeech { reason } => listener.on_no_speech(reason),
            EngineEventWire::Preview { text, .. } | EngineEventWire::Correction { text, .. } => {
                listener.on_preview(text)
            }
            EngineEventWire::UtteranceFinal { text, .. } => listener.on_final(text),
            EngineEventWire::Warning { code, message } => {
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

async fn optimistically_show_overlay(event: &HotkeyEvent, controller: &RecordingController) {
    let starts_redesign_overlay = matches!(
        event,
        HotkeyEvent::ToggleNormal
            | HotkeyEvent::ToggleRaw
            | HotkeyEvent::Hold {
                action: HoldAction::Down,
                ..
            }
    );
    if !starts_redesign_overlay || controller.current_state().await != State::Idle {
        return;
    }
    if let Some(listener) = current_listener() {
        listener.on_recording_started();
    }
}

/// Process-global hotkey runtime owner.
///
/// `start()` installs the native listener but creates `RecordingController`
/// lazily on the first real hotkey event. That keeps app launch/menu-open free
/// of `Config::load()` side effects while still routing hotkeys through the
/// real controller once the user intentionally invokes a shortcut.
#[derive(uniffi::Object)]
pub struct CodescribeHotkeys {}

#[uniffi::export(async_runtime = "tokio")]
impl CodescribeHotkeys {
    #[uniffi::constructor]
    pub fn new() -> Self {
        Self {}
    }

    /// Start or replace the process-global hotkey listener.
    pub async fn start(&self) -> Result<(), CsError> {
        let (tx, rx) = unbounded::<HotkeyEvent>();
        let handle = tokio::runtime::Handle::current();
        let controller_store = shared_controller();

        hotkeys::install_global_hotkey_manager(tx.clone())
            .map_err(|msg| CsError::Recording { msg })?;

        std::thread::spawn(move || {
            for event in rx {
                let controller = ensure_controller(&controller_store, handle.clone());
                let handle = handle.clone();
                handle.spawn(async move {
                    optimistically_show_overlay(&event, &controller).await;
                    if let Err(error) = dispatch_hotkey_event(event, controller).await {
                        eprintln!("Hotkey event error: {error}");
                    }
                });
            }
        });

        Ok(())
    }

    /// Register the Swift overlay listener for the shared controller event stream.
    pub fn set_listener(&self, listener: Arc<dyn CsTranscriptionListener>) {
        let listener_store = shared_listener();
        let mut guard = listener_store.write().unwrap_or_else(|e| e.into_inner());
        *guard = Some(listener);
    }

    /// Start the same toggle recording flow used by the default hotkey.
    pub async fn start_recording(&self) -> Result<(), CsError> {
        let controller = ensure_controller(&shared_controller(), tokio::runtime::Handle::current());
        let event = HotkeyEvent::ToggleNormal;
        optimistically_show_overlay(&event, &controller).await;
        dispatch_hotkey_event(event, controller)
            .await
            .map_err(|error| CsError::Recording {
                msg: error.to_string(),
            })
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

    /// Stop the global hotkey listener if it is active.
    pub fn stop(&self) {
        hotkeys::shutdown_global_hotkey_manager();
    }

    /// True once the listener is installed and owned by this process.
    pub fn is_active(&self) -> bool {
        hotkeys::is_global_hotkey_manager_active()
    }
}

async fn dispatch_hotkey_event(
    event: HotkeyEvent,
    controller: Arc<RecordingController>,
) -> anyhow::Result<()> {
    match event {
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
            let _ =
                codescribe::tray::update_tray_status(codescribe::tray::TrayStatus::HotkeyConflict);
            codescribe::os::notifications::notify("Codescribe hotkey conflict", &body);
        }
    }

    Ok(())
}
