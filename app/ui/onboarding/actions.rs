//! Wizard flow control: button actions, step advance/retreat, auto-advance
//! scheduling, full-disk polling, persistence of user choices, and the
//! finish/teardown path.

use std::thread;
use std::time::Duration;

use dispatch::Queue;
use objc::{msg_send, sel, sel_impl};
use tracing::{info, warn};

use crate::config::{Config, ShortcutBinding, UserSettings, WorkMode, keychain};
use crate::os::hotkeys;
use crate::os::permissions::PermissionStatus;
use crate::ui::shared::helpers::{set_text_field_string, window_close};

use super::Id;
use super::permission_flow::{
    PermissionUiStatus, check_permission_state, permission_status,
    reconcile_permission_runtime_after_grant, reconcile_runtime_after_onboarding_completion,
    request_permission, should_wait_for_restart,
};
use super::render::render_current_step;
use super::session::{mark_onboarding_done, release_onboarding_lock, save_onboarding_progress};
use super::state::{HotkeyModeChoice, ONBOARDING_STATE};
use super::steps::{PermissionKind, TOTAL_STEPS, WizardStep, step_for_index};
use super::widgets::{get_text_field_string, system_green_color, system_red_color};

const FULL_DISK_STEP_INDEX: usize = 5;

pub(super) fn handle_primary_action() {
    let step = {
        let state = ONBOARDING_STATE.lock().unwrap_or_else(|e| e.into_inner());
        step_for_index(state.step_index)
    };

    match step {
        WizardStep::Welcome => advance_step(),
        WizardStep::Permission(kind) => handle_permission_primary(kind),
        WizardStep::Language => {
            save_language_choice();
            advance_step();
        }
        WizardStep::ApiKey => {
            if persist_api_key_from_field() {
                advance_step();
            }
        }
        WizardStep::HotkeyMode => {
            save_hotkey_mode();
            advance_step();
        }
        WizardStep::Done => finish_onboarding(true),
    }
}

pub(super) fn handle_back_action() {
    retreat_step();
}

pub(super) fn handle_skip_action() {
    let step = {
        let state = ONBOARDING_STATE.lock().unwrap_or_else(|e| e.into_inner());
        step_for_index(state.step_index)
    };

    match step {
        WizardStep::Permission(PermissionKind::FullDiskAccess) => {
            let mut state = ONBOARDING_STATE.lock().unwrap_or_else(|e| e.into_inner());
            state.full_disk_polling = false;
            drop(state);
            advance_step();
        }
        WizardStep::ApiKey => {
            mark_api_key_skipped();
            advance_step();
        }
        _ => finish_onboarding(false),
    }
}

fn handle_permission_primary(kind: PermissionKind) {
    let idx = kind.index();
    let step_to_persist;
    let already_requested;

    {
        let mut state = ONBOARDING_STATE.lock().unwrap_or_else(|e| e.into_inner());
        let status = state.permission_states[idx];
        let requested = state.requested_permissions[idx];
        if status == PermissionUiStatus::Granted {
            drop(state);
            if should_wait_for_restart(kind, status, requested) {
                finish_onboarding(false);
            } else {
                advance_step();
            }
            return;
        }

        already_requested = requested;
        state.requested_permissions[idx] = true;
        step_to_persist = state.step_index;
    }

    if kind == PermissionKind::FullDiskAccess && already_requested {
        let _ = request_permission(kind);
        start_full_disk_polling();

        {
            let mut state = ONBOARDING_STATE.lock().unwrap_or_else(|e| e.into_inner());
            let requested = state.requested_permissions[idx];
            state.permission_states[idx] = check_permission_state(kind, requested);
        }

        render_current_step();
        return;
    }

    // Persist checkpoint before asking TCC in case macOS forces an app restart.
    save_onboarding_progress(step_to_persist);

    if kind == PermissionKind::Microphone {
        thread::spawn(move || {
            let _ = request_permission(kind);
            Queue::main().exec_async(move || {
                let mut should_render = false;
                let mut should_schedule = false;
                {
                    let mut state = ONBOARDING_STATE.lock().unwrap_or_else(|e| e.into_inner());
                    let requested = state.requested_permissions[idx];
                    state.permission_states[idx] = check_permission_state(kind, requested);
                    if state.step_index == step_to_persist {
                        should_render = true;
                        if state.permission_states[idx] == PermissionUiStatus::Granted {
                            reconcile_permission_runtime_after_grant(kind);
                            should_schedule = true;
                        }
                    }
                }

                if should_render {
                    render_current_step();
                }
                if should_schedule {
                    maybe_schedule_auto_advance(step_to_persist);
                }
            });
        });

        // Keep UI responsive while system prompt is in flight.
        render_current_step();
        return;
    }

    let _ = request_permission(kind);

    if kind == PermissionKind::FullDiskAccess {
        start_full_disk_polling();
    }

    {
        let mut state = ONBOARDING_STATE.lock().unwrap_or_else(|e| e.into_inner());
        let requested = state.requested_permissions[idx];
        state.permission_states[idx] = check_permission_state(kind, requested);
    }

    if permission_status(kind) == PermissionStatus::Granted {
        reconcile_permission_runtime_after_grant(kind);
    }

    render_current_step();
}

fn advance_step() {
    let mut should_render = false;
    let mut new_step = None;
    {
        let mut state = ONBOARDING_STATE.lock().unwrap_or_else(|e| e.into_inner());
        if state.step_index + 1 < TOTAL_STEPS {
            state.step_index += 1;
            state.scheduled_auto_advance_step = None;
            if state.step_index != FULL_DISK_STEP_INDEX {
                state.full_disk_polling = false;
            }
            new_step = Some(state.step_index);
            should_render = true;
        }
    }
    if let Some(step) = new_step {
        save_onboarding_progress(step);
    }

    if should_render {
        render_current_step();
    }
}

fn retreat_step() {
    let mut should_render = false;
    let mut new_step = None;
    {
        let mut state = ONBOARDING_STATE.lock().unwrap_or_else(|e| e.into_inner());
        if state.step_index > 0 {
            state.step_index -= 1;
            state.scheduled_auto_advance_step = None;
            if state.step_index != FULL_DISK_STEP_INDEX {
                state.full_disk_polling = false;
            }
            new_step = Some(state.step_index);
            should_render = true;
        }
    }
    if let Some(step) = new_step {
        save_onboarding_progress(step);
    }

    if should_render {
        render_current_step();
    }
}

pub(super) fn maybe_schedule_auto_advance(step_index: usize) {
    let should_schedule = {
        let mut state = ONBOARDING_STATE.lock().unwrap_or_else(|e| e.into_inner());
        if state.scheduled_auto_advance_step == Some(step_index) {
            false
        } else {
            state.scheduled_auto_advance_step = Some(step_index);
            true
        }
    };

    if !should_schedule {
        return;
    }

    thread::spawn(move || {
        thread::sleep(Duration::from_millis(800));
        Queue::main().exec_async(move || {
            let mut should_advance = false;

            {
                let mut state = ONBOARDING_STATE.lock().unwrap_or_else(|e| e.into_inner());
                if state.step_index == step_index
                    && let WizardStep::Permission(kind) = step_for_index(step_index)
                {
                    let idx = kind.index();
                    let requested = state.requested_permissions[idx];
                    let status = check_permission_state(kind, requested);
                    state.permission_states[idx] = status;
                    if status == PermissionUiStatus::Granted
                        && !should_wait_for_restart(kind, status, requested)
                    {
                        should_advance = true;
                    }
                }
                state.scheduled_auto_advance_step = None;
            }

            if should_advance {
                advance_step();
            } else {
                render_current_step();
            }
        });
    });
}

fn start_full_disk_polling() {
    let should_start = {
        let mut state = ONBOARDING_STATE.lock().unwrap_or_else(|e| e.into_inner());
        if state.full_disk_polling {
            false
        } else {
            state.full_disk_polling = true;
            true
        }
    };

    if !should_start {
        return;
    }

    thread::spawn(|| {
        loop {
            thread::sleep(Duration::from_secs(2));

            let keep_running = {
                let state = ONBOARDING_STATE.lock().unwrap_or_else(|e| e.into_inner());
                state.full_disk_polling
            };

            if !keep_running {
                break;
            }

            Queue::main().exec_async(|| {
                let mut should_schedule = false;
                let mut should_render = false;

                {
                    let mut state = ONBOARDING_STATE.lock().unwrap_or_else(|e| e.into_inner());
                    if step_for_index(state.step_index)
                        == WizardStep::Permission(PermissionKind::FullDiskAccess)
                    {
                        let idx = PermissionKind::FullDiskAccess.index();
                        state.permission_states[idx] =
                            check_permission_state(PermissionKind::FullDiskAccess, true);
                        let granted = state.permission_states[idx] == PermissionUiStatus::Granted;
                        should_schedule = granted
                            && !should_wait_for_restart(
                                PermissionKind::FullDiskAccess,
                                state.permission_states[idx],
                                state.requested_permissions[idx],
                            );
                        if granted {
                            state.full_disk_polling = false;
                        }
                        should_render = true;
                    } else {
                        state.full_disk_polling = false;
                    }
                }

                if should_render {
                    render_current_step();
                }
                if should_schedule {
                    maybe_schedule_auto_advance(FULL_DISK_STEP_INDEX);
                }
            });
        }
    });
}

fn save_language_choice() {
    let language = {
        let state = ONBOARDING_STATE.lock().unwrap_or_else(|e| e.into_inner());
        state.language
    };

    let mut settings = UserSettings::load();
    settings.whisper_language = Some(language.value().to_string());
    if let Err(e) = settings.save() {
        warn!(
            "Onboarding: failed to persist language {}: {e}",
            language.value()
        );
    }

    unsafe { std::env::set_var("WHISPER_LANGUAGE", language.value()) };
    info!("Onboarding: language set to {}", language.value());
}

fn persist_api_key_from_field() -> bool {
    let key = {
        let state = ONBOARDING_STATE.lock().unwrap_or_else(|e| e.into_inner());
        state
            .ui
            .api_key_field
            .map(|ptr| get_text_field_string(ptr as Id))
            .unwrap_or_default()
            .trim()
            .to_string()
    };

    if key.is_empty() {
        mark_api_key_skipped();
        return true;
    }

    match keychain::save_key("LLM_FORMATTING_API_KEY", &key).and_then(|_| {
        keychain::save_key("LLM_ASSISTIVE_API_KEY", &key).inspect_err(|_| {
            let _ = keychain::delete_key("LLM_FORMATTING_API_KEY");
        })
    }) {
        Ok(()) => {
            unsafe {
                std::env::set_var("LLM_FORMATTING_API_KEY", &key);
                std::env::set_var("LLM_ASSISTIVE_API_KEY", &key);
            };
            let mut settings = UserSettings::load();
            settings.ai_formatting_enabled = Some(true);
            if let Err(e) = settings.save() {
                warn!("Onboarding: failed to persist AI formatting setting: {e}");
            }

            unsafe { std::env::set_var("AI_FORMATTING_ENABLED", "1") };

            let mut state = ONBOARDING_STATE.lock().unwrap_or_else(|e| e.into_inner());
            state.api_key_configured = true;

            if let Some(label_ptr) = state.ui.api_hint_label {
                unsafe {
                    set_text_field_string(label_ptr as Id, "OpenAI API key saved to Keychain.");
                    let green = system_green_color();
                    let _: () = msg_send![label_ptr as Id, setTextColor: green];
                }
            }
            true
        }
        Err(e) => {
            warn!("Onboarding: failed to save API key: {e}");
            let state = ONBOARDING_STATE
                .lock()
                .unwrap_or_else(|poison| poison.into_inner());
            if let Some(label_ptr) = state.ui.api_hint_label {
                unsafe {
                    set_text_field_string(label_ptr as Id, "Failed to save key. Please try again.");
                    let red = system_red_color();
                    let _: () = msg_send![label_ptr as Id, setTextColor: red];
                }
            }
            false
        }
    }
}

fn mark_api_key_skipped() {
    let mut settings = UserSettings::load();
    settings.ai_formatting_enabled = Some(false);
    if let Err(e) = settings.save() {
        warn!("Onboarding: failed to persist AI formatting disabled state: {e}");
    }

    unsafe { std::env::set_var("AI_FORMATTING_ENABLED", "0") };

    let mut state = ONBOARDING_STATE.lock().unwrap_or_else(|e| e.into_inner());
    state.api_key_configured = false;
}

fn save_hotkey_mode() {
    let mode = {
        let state = ONBOARDING_STATE.lock().unwrap_or_else(|e| e.into_inner());
        state.hotkey_mode
    };

    let (dictation, formatting, assistive) = match mode {
        HotkeyModeChoice::HoldToTalk => (
            ShortcutBinding::HoldFn,
            ShortcutBinding::Disabled,
            ShortcutBinding::Disabled,
        ),
        HotkeyModeChoice::Toggle => (
            ShortcutBinding::Disabled,
            ShortcutBinding::DoubleLeftOption,
            ShortcutBinding::DoubleRightOption,
        ),
        HotkeyModeChoice::Both => (
            ShortcutBinding::HoldFn,
            ShortcutBinding::DoubleLeftOption,
            ShortcutBinding::DoubleRightOption,
        ),
    };

    let mut settings = UserSettings::load();
    settings.set_mode_binding(WorkMode::Dictation, dictation);
    settings.set_mode_binding(WorkMode::Formatting, formatting);
    settings.set_mode_binding(WorkMode::Assistive, assistive);

    hotkeys::apply_hotkey_config(&Config::load());

    info!("Onboarding: hotkey mode set to {}", mode.label());
}

/// Persist the selected first-run operating lane (Basic / Agentic) into
/// settings.json, mirroring [`save_language_choice`]. No wizard step mutates
/// the lane yet, so a completed onboarding records the safe Basic default; a
/// later readiness-UI cut will set `state.onboarding_mode` before finish.
fn save_onboarding_mode() {
    let mode = {
        let state = ONBOARDING_STATE.lock().unwrap_or_else(|e| e.into_inner());
        state.onboarding_mode
    };

    let mut settings = UserSettings::load();
    settings.onboarding_mode = Some(mode.value().to_string());
    if let Err(e) = settings.save() {
        warn!(
            "Onboarding: failed to persist onboarding mode {}: {e}",
            mode.value()
        );
    }

    info!("Onboarding: mode set to {}", mode.label());
}

pub(super) fn finish_onboarding(completed: bool) {
    if completed {
        save_onboarding_mode();
        reconcile_runtime_after_onboarding_completion();
        mark_onboarding_done();
    }

    let window_ptr = {
        let mut state = ONBOARDING_STATE.lock().unwrap_or_else(|e| e.into_inner());
        state.full_disk_polling = false;
        state.scheduled_auto_advance_step = None;
        state.closing_via_finish = true;
        state.window.take()
    };

    if let Some(ptr) = window_ptr {
        unsafe { window_close(ptr as Id) };
    } else {
        let mut state = ONBOARDING_STATE.lock().unwrap_or_else(|e| e.into_inner());
        state.closing_via_finish = false;
        drop(state);
        release_onboarding_lock();
    }
}
