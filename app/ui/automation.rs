use std::time::Duration;

use codescribe_core::ipc::{AppAutomationAction, AppAutomationState};
use tokio::time::sleep;

use crate::config::Config;
use crate::ui::overlay::{
    hide_transcription_overlay, is_transcription_overlay_visible, show_transcription_overlay,
};
use crate::ui::settings::{hide_settings_window, is_settings_window_visible, show_settings_window};
use crate::ui::tray;
use crate::ui::tray::handlers::{
    handle_continue_onboarding_action, handle_open_settings_action, handle_show_agent_action,
};
use crate::ui::voice_chat::{
    hide_voice_chat_overlay, is_voice_chat_overlay_visible, show_voice_chat_overlay,
};

const AUTOMATION_TIMEOUT_MS: u64 = 1_500;
const AUTOMATION_POLL_INTERVAL_MS: u64 = 25;

pub fn app_automation_state() -> AppAutomationState {
    AppAutomationState {
        settings_visible: is_settings_window_visible(),
        voice_chat_visible: is_voice_chat_overlay_visible(),
        transcription_overlay_visible: is_transcription_overlay_visible(),
        setup_required: crate::ui::onboarding::should_show_onboarding(),
        dock_icon_visible: Config::load().show_dock_icon,
    }
}

pub async fn run_app_automation(action: AppAutomationAction) -> Result<AppAutomationState, String> {
    dispatch_action(action);

    match action {
        AppAutomationAction::ResetUi => {
            wait_for_state("all app surfaces hidden", |state| {
                !state.settings_visible
                    && !state.voice_chat_visible
                    && !state.transcription_overlay_visible
            })
            .await
        }
        AppAutomationAction::ShowSettings
        | AppAutomationAction::TriggerTrayOpenSettings
        | AppAutomationAction::TriggerTrayContinueOnboarding
        | AppAutomationAction::TriggerDockReopen => {
            wait_for_state("settings visible", |state| state.settings_visible).await
        }
        AppAutomationAction::HideSettings => {
            wait_for_state("settings hidden", |state| !state.settings_visible).await
        }
        AppAutomationAction::ShowVoiceChat | AppAutomationAction::TriggerTrayShowAgent => {
            wait_for_state("voice chat visible", |state| state.voice_chat_visible).await
        }
        AppAutomationAction::HideVoiceChat => {
            wait_for_state("voice chat hidden", |state| !state.voice_chat_visible).await
        }
        AppAutomationAction::ShowTranscriptionOverlay => {
            wait_for_state("transcription overlay visible", |state| {
                state.transcription_overlay_visible
            })
            .await
        }
        AppAutomationAction::HideTranscriptionOverlay => {
            wait_for_state("transcription overlay hidden", |state| {
                !state.transcription_overlay_visible
            })
            .await
        }
    }
}

fn dispatch_action(action: AppAutomationAction) {
    match action {
        AppAutomationAction::ResetUi => {
            hide_transcription_overlay();
            hide_voice_chat_overlay();
            hide_settings_window();
        }
        AppAutomationAction::ShowSettings => show_settings_window(),
        AppAutomationAction::HideSettings => hide_settings_window(),
        AppAutomationAction::ShowVoiceChat => show_voice_chat_overlay(),
        AppAutomationAction::HideVoiceChat => hide_voice_chat_overlay(),
        AppAutomationAction::ShowTranscriptionOverlay => show_transcription_overlay(),
        AppAutomationAction::HideTranscriptionOverlay => hide_transcription_overlay(),
        AppAutomationAction::TriggerTrayShowAgent => handle_show_agent_action(),
        AppAutomationAction::TriggerTrayOpenSettings => handle_open_settings_action(),
        AppAutomationAction::TriggerTrayContinueOnboarding => handle_continue_onboarding_action(),
        AppAutomationAction::TriggerDockReopen => tray::handle_dock_reopen(),
    }
}

async fn wait_for_state(
    label: &str,
    predicate: impl Fn(&AppAutomationState) -> bool,
) -> Result<AppAutomationState, String> {
    let attempts = (AUTOMATION_TIMEOUT_MS / AUTOMATION_POLL_INTERVAL_MS).max(1);
    for _ in 0..=attempts {
        let state = app_automation_state();
        if predicate(&state) {
            return Ok(state);
        }
        sleep(Duration::from_millis(AUTOMATION_POLL_INTERVAL_MS)).await;
    }

    Err(format!(
        "Timed out waiting for {label}; last state: {:?}",
        app_automation_state()
    ))
}
