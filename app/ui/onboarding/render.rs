//! Per-step rendering: headline copy, status pills, sidebar markers, and the
//! final summary view. Pure read-modify of already-built UI elements.

use crate::ui::shared::helpers::color_label;

use super::actions::maybe_schedule_auto_advance;
use super::permission_flow::{
    PERMISSION_ORDER, PermissionUiStatus, check_permission_state, permission_instruction_text,
    permission_status_color, permission_status_text, refresh_all_permission_states_locked,
    should_wait_for_restart,
};
use super::state::{HotkeyModeChoice, LanguageChoice, ONBOARDING_STATE, UiRefs};
use super::steps::{
    PermissionKind, PermissionRecoveryStrategy, TOTAL_STEPS, WizardStep, step_for_index,
};
use super::widgets::{
    set_button_title_if_present, set_hidden_if_present, set_label_color_if_present,
    set_text_if_present, sync_hotkey_radios, sync_language_radios, system_green_color,
    system_red_color, system_secondary_color,
};

pub(super) fn render_current_step() {
    let (
        step_index,
        step,
        language,
        hotkey_mode,
        api_key_configured,
        permissions,
        requested_permissions,
        ui,
    ) = {
        let mut state = ONBOARDING_STATE.lock().unwrap_or_else(|e| e.into_inner());
        let step = step_for_index(state.step_index);
        match step {
            WizardStep::Permission(kind) => {
                let idx = kind.index();
                let requested = state.requested_permissions[idx];
                state.permission_states[idx] = check_permission_state(kind, requested);
            }
            WizardStep::Done => {
                refresh_all_permission_states_locked(&mut state);
            }
            _ => {}
        }

        (
            state.step_index,
            step,
            state.language,
            state.hotkey_mode,
            state.api_key_configured,
            state.permission_states,
            state.requested_permissions,
            state.ui,
        )
    };

    set_text_if_present(
        ui.step_counter_label,
        &format!("Step {} of {}", step_index + 1, TOTAL_STEPS),
    );

    set_hidden_if_present(ui.status_label, true);
    set_hidden_if_present(ui.instruction_label, true);
    set_hidden_if_present(ui.language_view, true);
    set_hidden_if_present(ui.api_view, true);
    set_hidden_if_present(ui.hotkey_view, true);
    set_hidden_if_present(ui.summary_view, true);
    set_hidden_if_present(ui.skip_button, matches!(step, WizardStep::Done));
    if step != WizardStep::Done {
        set_button_title_if_present(ui.skip_button, "Not Now");
    }

    set_hidden_if_present(ui.back_button, step_index == 0);

    sync_language_radios(ui, language);
    sync_hotkey_radios(ui, hotkey_mode);
    update_sidebar_step_labels(ui, step_index, permissions);

    match step {
        WizardStep::Welcome => {
            set_text_if_present(ui.icon_label, "WELCOME");
            set_text_if_present(ui.title_label, "Welcome to CodeScribe");
            set_text_if_present(
                ui.description_label,
                "We will wire permissions, choose your transcript defaults, and show how live preview, committed verdict, and AI help stay honest from first launch.",
            );
            set_button_title_if_present(ui.primary_button, "Get Started");
        }
        WizardStep::Permission(kind) => {
            let idx = kind.index();
            let status = permissions[idx];
            let requested = requested_permissions[idx];
            set_text_if_present(ui.icon_label, kind.icon());
            set_text_if_present(ui.title_label, kind.title());
            set_text_if_present(ui.description_label, kind.reason());

            set_hidden_if_present(ui.status_label, false);
            set_text_if_present(
                ui.status_label,
                permission_status_text(kind, status, requested),
            );
            set_label_color_if_present(ui.status_label, permission_status_color(status));

            if status == PermissionUiStatus::Granted {
                if should_wait_for_restart(kind, status, requested) {
                    set_button_title_if_present(ui.primary_button, "Close for Restart");
                } else {
                    set_button_title_if_present(ui.primary_button, "Continue");
                    maybe_schedule_auto_advance(step_index);
                }
            } else if kind == PermissionKind::FullDiskAccess {
                set_button_title_if_present(ui.primary_button, "Open Settings");
                set_hidden_if_present(ui.skip_button, false);
                set_button_title_if_present(
                    ui.skip_button,
                    if requested {
                        "Continue Without It"
                    } else {
                        "Skip"
                    },
                );
            } else {
                set_button_title_if_present(
                    ui.primary_button,
                    if kind.recovery_strategy() == PermissionRecoveryStrategy::AppRestartRequired
                        && requested
                    {
                        "Open Settings"
                    } else if status == PermissionUiStatus::Denied {
                        "Try Again"
                    } else {
                        "Grant Access"
                    },
                );
            }

            if let Some(text) = permission_instruction_text(kind, status, requested) {
                set_hidden_if_present(ui.instruction_label, false);
                set_text_if_present(ui.instruction_label, text);
            }
        }
        WizardStep::Language => {
            set_text_if_present(ui.icon_label, "LANG");
            set_text_if_present(ui.title_label, "Choose Language");
            set_text_if_present(
                ui.description_label,
                "Select the default transcript language. Live preview stays provisional, and the committed verdict will use this language unless you change it later in Settings.",
            );
            set_hidden_if_present(ui.language_view, false);
            set_button_title_if_present(ui.primary_button, "Continue");
        }
        WizardStep::ApiKey => {
            set_text_if_present(ui.icon_label, "API");
            set_text_if_present(ui.title_label, "Add OpenAI API Key");
            set_text_if_present(
                ui.description_label,
                "Put your OpenAI API key here to unlock formatting and the dictation-driven agent. Raw local transcript still works if you skip.",
            );
            set_hidden_if_present(ui.api_view, false);
            set_button_title_if_present(ui.primary_button, "Save & Continue");
            set_hidden_if_present(ui.skip_button, false);
            set_button_title_if_present(ui.skip_button, "Skip OpenAI");
        }
        WizardStep::HotkeyMode => {
            set_text_if_present(ui.icon_label, "HOTKEY");
            set_text_if_present(ui.title_label, "Mode Shortcuts");
            set_text_if_present(
                ui.description_label,
                "Mode first, keys second. Dictation aims for a committed transcript verdict, Formatting upgrades text only when safe, and Assistive stays in the chat overlay instead of silent paste.",
            );
            set_hidden_if_present(ui.hotkey_view, false);
            set_button_title_if_present(ui.primary_button, "Continue");
        }
        WizardStep::Done => {
            set_text_if_present(ui.icon_label, "DONE");
            set_text_if_present(ui.title_label, "You're All Set");
            // Short copy here on purpose: the summary_view below renders 4 lines
            // of permission status + config. With a longer description the two
            // overlap (description y=268..372 vs summary first-row y≈332).
            set_text_if_present(
                ui.description_label,
                "Truth model below — adjust later in Settings.",
            );
            set_hidden_if_present(ui.summary_view, false);
            set_hidden_if_present(ui.skip_button, true);
            update_summary_view(ui, permissions, language, api_key_configured, hotkey_mode);
            set_button_title_if_present(ui.primary_button, "Start CodeScribe");
        }
    }
}

fn sidebar_step_title(step: WizardStep) -> &'static str {
    match step {
        WizardStep::Welcome => "Welcome",
        WizardStep::Permission(PermissionKind::Microphone) => "Microphone",
        WizardStep::Permission(PermissionKind::Accessibility) => "Accessibility",
        WizardStep::Permission(PermissionKind::InputMonitoring) => "Input Monitoring",
        WizardStep::Permission(PermissionKind::ScreenRecording) => "Screen Recording",
        WizardStep::Permission(PermissionKind::FullDiskAccess) => "Full Disk Access",
        WizardStep::Language => "Language",
        WizardStep::ApiKey => "API Key",
        WizardStep::HotkeyMode => "Hotkeys",
        WizardStep::Done => "Finish",
    }
}

fn update_sidebar_step_labels(
    ui: UiRefs,
    current_step_index: usize,
    permissions: [PermissionUiStatus; 5],
) {
    for idx in 0..TOTAL_STEPS {
        let step = step_for_index(idx);
        let (marker, color) = if idx == current_step_index {
            if let WizardStep::Permission(kind) = step {
                let status = permissions[kind.index()];
                if status == PermissionUiStatus::Denied {
                    ("\u{2715}", system_red_color())
                } else {
                    ("\u{25CF}", color_label())
                }
            } else {
                ("\u{25CF}", color_label())
            }
        } else if idx < current_step_index {
            if let WizardStep::Permission(PermissionKind::FullDiskAccess) = step {
                if permissions[PermissionKind::FullDiskAccess.index()]
                    != PermissionUiStatus::Granted
                {
                    ("\u{2013}", system_secondary_color())
                } else {
                    ("\u{2713}", system_green_color())
                }
            } else {
                ("\u{2713}", system_green_color())
            }
        } else {
            ("\u{25CB}", system_secondary_color())
        };

        let text = format!("{marker} {}", sidebar_step_title(step));
        set_text_if_present(ui.sidebar_step_labels[idx], &text);
        set_label_color_if_present(ui.sidebar_step_labels[idx], color);
    }
}

fn update_summary_view(
    ui: UiRefs,
    statuses: [PermissionUiStatus; 5],
    language: LanguageChoice,
    api_key_configured: bool,
    hotkey_mode: HotkeyModeChoice,
) {
    for kind in PERMISSION_ORDER {
        let idx = kind.index();
        let text = if statuses[idx] == PermissionUiStatus::Granted {
            format!("\u{2713} {}", kind.title())
        } else {
            format!("\u{2715} {}", kind.title())
        };
        set_text_if_present(ui.summary_permission_labels[idx], &text);

        let color = if statuses[idx] == PermissionUiStatus::Granted {
            system_green_color()
        } else {
            system_red_color()
        };
        set_label_color_if_present(ui.summary_permission_labels[idx], color);
    }

    let api_status = if api_key_configured {
        "OpenAI key configured"
    } else {
        "OpenAI key not configured"
    };

    set_text_if_present(
        ui.summary_config_label,
        &format!(
            "Language: {}\nOpenAI: {}\nMode profile: {}\nTruth model: Live preview stays local and provisional. CodeScribe only commits a final verdict after capture, and degraded fallback blocks silent auto-paste.",
            language.label(),
            api_status,
            hotkey_mode.label()
        ),
    );
}
