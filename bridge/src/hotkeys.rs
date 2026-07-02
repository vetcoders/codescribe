//! Global hotkey runtime surface for the SwiftUI redesign.
//!
//! This does not reimplement hotkeys in Swift. It starts the same macOS
//! `CGEventTap` listener used by the legacy daemon and dispatches emitted
//! `HotkeyEvent`s into the existing `RecordingController` state machine.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock, RwLock};

use codescribe::controller::{HotkeyAction, HotkeyInput, HotkeyType, RecordingController, State};
use codescribe::os::hotkeys::{self, HoldAction, HoldMode, HotkeyEvent};
use codescribe::os::shortcut_registry::{detect_hotkey_conflicts, fn_tap_intercept_note};
use codescribe_core::config::{Config, ModeBinding, ShortcutBinding, UserSettings, WorkMode};
use codescribe_core::ipc::{EngineEventWire, IpcEventPayload};
use crossbeam_channel::unbounded;
use tokio::runtime::Handle;
use tokio::sync::broadcast::error::RecvError;

use crate::recording::{CsAnnotationKind, CsLayerSummary, CsTranscriptionListener};
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
                listener.on_recording_started();
            }
            "idle" => {
                PREPARING_PENDING.store(false, Ordering::Release);
                listener.on_recording_stopped();
            }
            _ => {}
        },
        IpcEventPayload::FinalTranscript { text } => listener.on_final_transcript_ready(text),
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
    if PREPARING_PENDING.swap(false, Ordering::AcqRel)
        && let Some(listener) = current_listener()
    {
        listener.on_recording_stopped();
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
        let controller_store = shared_controller();

        hotkeys::install_global_hotkey_manager(tx.clone())
            .map_err(|msg| CsError::Recording { msg })?;

        std::thread::spawn(move || {
            for event in rx {
                let spawn_handle = handle.clone();
                let controller_handle = handle.clone();
                let controller_store = Arc::clone(&controller_store);
                spawn_handle.spawn(async move {
                    optimistically_show_overlay(&event).await;
                    let controller = ensure_controller(&controller_store, controller_handle);
                    let dispatch = dispatch_hotkey_event(event, Arc::clone(&controller)).await;
                    compensate_orphaned_preparing(&controller).await;
                    if let Err(error) = dispatch {
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
        let event = HotkeyEvent::ToggleNormal;
        optimistically_show_overlay(&event).await;
        let controller = ensure_controller(&shared_controller(), tokio::runtime::Handle::current());
        let dispatch = dispatch_hotkey_event(event, Arc::clone(&controller)).await;
        compensate_orphaned_preparing(&controller).await;
        dispatch.map_err(|error| CsError::Recording {
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
            codescribe::os::tray_status::update_tray_status(
                codescribe::os::tray_status::TrayStatus::HotkeyConflict,
            );
            codescribe::os::notifications::notify("Codescribe hotkey conflict", &body);
        }
    }

    Ok(())
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

#[uniffi::export]
impl CodescribeHotkeys {
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

    /// Paths 1 & 2 (quick hold-release cancel, start-failure reset): preparing was
    /// shown, the controller ended the dispatch at Idle with no broadcast → the
    /// compensator must emit exactly one terminal stop.
    #[tokio::test]
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
}
