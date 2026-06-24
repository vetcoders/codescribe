//! Wizard state: user choices, UI element references, and the global
//! onboarding state cell, plus probes that derive initial state from
//! persisted settings.

use std::sync::{LazyLock, Mutex};

use crate::config::{ShortcutBinding, UserSettings, WorkMode, keychain};

use super::permission_flow::PermissionUiStatus;
use super::steps::TOTAL_STEPS;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(super) enum LanguageChoice {
    #[default]
    English,
    Polish,
}

impl LanguageChoice {
    pub(super) fn label(self) -> &'static str {
        match self {
            Self::English => "English",
            Self::Polish => "Polish",
        }
    }

    pub(super) fn value(self) -> &'static str {
        match self {
            Self::English => "en",
            Self::Polish => "pl",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(super) enum HotkeyModeChoice {
    HoldToTalk,
    Toggle,
    #[default]
    Both,
}

impl HotkeyModeChoice {
    pub(super) fn label(self) -> &'static str {
        match self {
            Self::HoldToTalk => "Dictation (Hold)",
            Self::Toggle => "Hands-off (Toggle)",
            Self::Both => "Hybrid",
        }
    }
}

#[derive(Clone, Copy, Default)]
pub(super) struct UiRefs {
    pub(super) sidebar_step_labels: [Option<usize>; TOTAL_STEPS],
    pub(super) icon_label: Option<usize>,
    pub(super) title_label: Option<usize>,
    pub(super) description_label: Option<usize>,
    pub(super) status_label: Option<usize>,
    pub(super) instruction_label: Option<usize>,
    pub(super) step_counter_label: Option<usize>,
    pub(super) primary_button: Option<usize>,
    pub(super) back_button: Option<usize>,
    pub(super) skip_button: Option<usize>,
    pub(super) language_view: Option<usize>,
    pub(super) language_en_radio: Option<usize>,
    pub(super) language_pl_radio: Option<usize>,
    pub(super) api_view: Option<usize>,
    pub(super) api_key_field: Option<usize>,
    pub(super) api_hint_label: Option<usize>,
    pub(super) hotkey_view: Option<usize>,
    pub(super) hotkey_hold_radio: Option<usize>,
    pub(super) hotkey_toggle_radio: Option<usize>,
    pub(super) hotkey_both_radio: Option<usize>,
    pub(super) summary_view: Option<usize>,
    pub(super) summary_permission_labels: [Option<usize>; 5],
    pub(super) summary_config_label: Option<usize>,
}

pub(super) struct OnboardingState {
    pub(super) window: Option<usize>,
    pub(super) window_delegate: Option<usize>,
    pub(super) action_handler: Option<usize>,
    pub(super) step_index: usize,
    pub(super) language: LanguageChoice,
    pub(super) hotkey_mode: HotkeyModeChoice,
    pub(super) requested_permissions: [bool; 5],
    pub(super) permission_states: [PermissionUiStatus; 5],
    pub(super) scheduled_auto_advance_step: Option<usize>,
    pub(super) full_disk_polling: bool,
    pub(super) closing_via_finish: bool,
    pub(super) api_key_configured: bool,
    pub(super) ui: UiRefs,
}

impl Default for OnboardingState {
    fn default() -> Self {
        Self {
            window: None,
            window_delegate: None,
            action_handler: None,
            step_index: 0,
            language: LanguageChoice::default(),
            hotkey_mode: HotkeyModeChoice::default(),
            requested_permissions: [false; 5],
            permission_states: [PermissionUiStatus::NotDetermined; 5],
            scheduled_auto_advance_step: None,
            full_disk_polling: false,
            closing_via_finish: false,
            api_key_configured: false,
            ui: UiRefs::default(),
        }
    }
}

pub(super) static ONBOARDING_STATE: LazyLock<Mutex<OnboardingState>> =
    LazyLock::new(|| Mutex::new(OnboardingState::default()));

pub(super) fn mode_api_key_configured() -> bool {
    ["LLM_FORMATTING_API_KEY", "LLM_ASSISTIVE_API_KEY"]
        .into_iter()
        .any(|account| {
            keychain::load_key(account)
                .map(|k| !k.trim().is_empty())
                .unwrap_or(false)
        })
}

pub(super) fn initial_language_choice() -> LanguageChoice {
    let settings = UserSettings::load();
    match settings.whisper_language.as_deref() {
        Some("pl") => LanguageChoice::Polish,
        _ => LanguageChoice::English,
    }
}

pub(super) fn initial_hotkey_choice() -> HotkeyModeChoice {
    let settings = UserSettings::load();
    let dictation = settings.mode_binding_for(WorkMode::Dictation);
    let formatting = settings.mode_binding_for(WorkMode::Formatting);
    let assistive = settings.mode_binding_for(WorkMode::Assistive);

    let hold_enabled = matches!(
        dictation,
        ShortcutBinding::HoldFn
            | ShortcutBinding::HoldCtrl
            | ShortcutBinding::HoldCtrlAlt
            | ShortcutBinding::HoldCtrlShift
            | ShortcutBinding::HoldCtrlCmd
    );
    let toggle_enabled = matches!(dictation, ShortcutBinding::DoubleCtrl)
        || formatting == ShortcutBinding::DoubleLeftOption
        || assistive == ShortcutBinding::DoubleRightOption;

    match (hold_enabled, toggle_enabled) {
        (true, true) => HotkeyModeChoice::Both,
        (true, false) => HotkeyModeChoice::HoldToTalk,
        (false, true) => HotkeyModeChoice::Toggle,
        (false, false) => HotkeyModeChoice::Both,
    }
}
