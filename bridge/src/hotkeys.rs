//! Global hotkey runtime surface for the SwiftUI redesign.
//!
//! This does not reimplement hotkeys in Swift. It starts the same macOS
//! `CGEventTap` listener used by the legacy daemon and dispatches emitted
//! `HotkeyEvent`s into the existing `RecordingController` state machine.

use std::sync::{Arc, Mutex, OnceLock, RwLock};

use codescribe::controller::{HotkeyAction, HotkeyInput, HotkeyType, RecordingController, State};
use codescribe::os::hotkeys::{self, HoldAction, HoldMode, HotkeyEvent};
use codescribe::os::shortcut_registry::{detect_hotkey_conflicts, fn_tap_intercept_note};
use codescribe_core::config::{Config, ModeBinding, ShortcutBinding, UserSettings, WorkMode};
use codescribe_core::ipc::{EngineEventWire, IpcEventPayload};
use crossbeam_channel::unbounded;
use tokio::runtime::Handle;

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
        listener.on_recording_preparing();
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
        codescribe::os::hotkeys::apply_hotkey_config(&codescribe_core::config::Config::load());

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
    hotkeys::apply_hotkey_config(&Config::load());
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
