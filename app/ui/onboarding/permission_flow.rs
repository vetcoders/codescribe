//! Permission status mapping, TCC request/recovery flow, and the runtime
//! reconciliation that re-wires hotkeys/microphone after a grant. This is the
//! permission contract shared with Settings (`crate::ui::settings`).

use tracing::{info, warn};

use crate::os::hotkeys;
use crate::os::permissions::{self, PermissionStatus};

use super::Id;
use super::state::OnboardingState;
use super::steps::{PermissionKind, PermissionRecoveryStrategy};
use super::widgets::{system_green_color, system_red_color, system_secondary_color};

const STATUS_NOT_DETERMINED: &str = "\u{25CB} Not Enabled Yet";
const STATUS_GRANTED: &str = "\u{25CF} Granted";
const STATUS_DENIED: &str = "\u{2715} Denied";
const STATUS_RESTART_REQUIRED: &str = "\u{25CF} Granted - Restart Required";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(super) enum PermissionUiStatus {
    #[default]
    NotDetermined,
    Granted,
    Denied,
}

pub(crate) const PERMISSION_ORDER: [PermissionKind; 5] = [
    PermissionKind::Microphone,
    PermissionKind::Accessibility,
    PermissionKind::InputMonitoring,
    PermissionKind::ScreenRecording,
    PermissionKind::FullDiskAccess,
];

pub(super) fn should_wait_for_restart(
    kind: PermissionKind,
    status: PermissionUiStatus,
    requested: bool,
) -> bool {
    kind.recovery_strategy() == PermissionRecoveryStrategy::AppRestartRequired
        && status == PermissionUiStatus::Granted
        && requested
}

pub(super) fn should_refresh_hotkey_runtime_after_grant(
    kind: PermissionKind,
    accessibility_status: PermissionStatus,
    input_monitoring_status: PermissionStatus,
) -> bool {
    matches!(
        kind,
        PermissionKind::Accessibility | PermissionKind::InputMonitoring
    ) && accessibility_status == PermissionStatus::Granted
        && input_monitoring_status == PermissionStatus::Granted
}

pub(crate) fn reconcile_permission_runtime_after_grant(kind: PermissionKind) {
    if permission_status(kind) != PermissionStatus::Granted {
        return;
    }

    match kind.recovery_strategy() {
        PermissionRecoveryStrategy::LiveRecheck => {
            if kind == PermissionKind::Microphone {
                crate::controller::request_permission_runtime_reconcile();
                info!(
                    "Onboarding: rechecked {} live after permission grant",
                    kind.runtime_subsystem()
                );
            }
        }
        PermissionRecoveryStrategy::LiveReinitialize => {
            let accessibility_status = permissions::check_accessibility();
            let input_monitoring_status = permissions::check_input_monitoring();
            if permissions::hotkey_permissions_granted()
                && should_refresh_hotkey_runtime_after_grant(
                    kind,
                    accessibility_status,
                    input_monitoring_status,
                )
            {
                // Dedup: if the global hotkey manager is already running (= a
                // prior permission grant in this same onboarding flow already
                // created it), skip the full teardown + restart cycle. CGEventTap
                // is process-global and remains attached across TCC re-checks —
                // the only case where we MUST refresh is the cold-start grant
                // when the manager was never created.
                if hotkeys::is_global_hotkey_manager_active() {
                    info!(
                        "Onboarding: {} granted; hotkey manager already running, skipping refresh (dedup)",
                        kind.runtime_subsystem()
                    );
                    return;
                }
                match hotkeys::refresh_global_hotkey_manager() {
                    Ok(()) => info!(
                        "Onboarding: reinitialized {} after permission grant",
                        kind.runtime_subsystem()
                    ),
                    Err(error) => warn!(
                        "Onboarding: failed to reinitialize {} after permission grant: {error}",
                        kind.runtime_subsystem()
                    ),
                }
            }
        }
        PermissionRecoveryStrategy::AppRestartRequired => {
            info!(
                "Onboarding: {} still requires app restart after grant",
                kind.runtime_subsystem()
            );
        }
    }
}

pub(super) fn reconcile_runtime_after_onboarding_completion() {
    for kind in [
        PermissionKind::Microphone,
        PermissionKind::Accessibility,
        PermissionKind::InputMonitoring,
    ] {
        reconcile_permission_runtime_after_grant(kind);
    }
}

pub(super) fn refresh_all_permission_states_locked(state: &mut OnboardingState) {
    for kind in PERMISSION_ORDER {
        let idx = kind.index();
        state.permission_states[idx] =
            check_permission_state(kind, state.requested_permissions[idx]);
    }
}

pub(crate) fn permission_status(kind: PermissionKind) -> PermissionStatus {
    match kind {
        PermissionKind::Microphone => permissions::check_microphone(),
        PermissionKind::Accessibility => permissions::check_accessibility(),
        PermissionKind::InputMonitoring => permissions::check_input_monitoring(),
        PermissionKind::ScreenRecording => permissions::check_screen_recording(),
        PermissionKind::FullDiskAccess => permissions::check_full_disk_access(),
    }
}

pub(super) fn check_permission_state(kind: PermissionKind, requested: bool) -> PermissionUiStatus {
    map_permission_status(kind, permission_status(kind), requested)
}

fn map_permission_status(
    kind: PermissionKind,
    status: PermissionStatus,
    requested: bool,
) -> PermissionUiStatus {
    match status {
        PermissionStatus::Granted => PermissionUiStatus::Granted,
        PermissionStatus::Denied => PermissionUiStatus::Denied,
        PermissionStatus::NotDetermined => {
            if requested && kind != PermissionKind::FullDiskAccess {
                PermissionUiStatus::Denied
            } else {
                PermissionUiStatus::NotDetermined
            }
        }
    }
}

fn permission_settings_deeplink(kind: PermissionKind) -> &'static str {
    match kind {
        PermissionKind::Microphone => "Privacy_Microphone",
        PermissionKind::Accessibility => "Privacy_Accessibility",
        PermissionKind::InputMonitoring => "Privacy_ListenEvent",
        PermissionKind::ScreenRecording => "Privacy_ScreenCapture",
        PermissionKind::FullDiskAccess => "Privacy_AllFiles",
    }
}

pub(crate) fn open_permission_settings(kind: PermissionKind) {
    permissions::open_privacy_settings(permission_settings_deeplink(kind));
}

pub(crate) fn request_permission(kind: PermissionKind) -> bool {
    match kind {
        PermissionKind::Microphone => {
            let result = permissions::request_microphone();
            if !result {
                open_permission_settings(kind);
            }
            result
        }
        PermissionKind::Accessibility => permissions::request_accessibility(),
        PermissionKind::InputMonitoring => permissions::request_input_monitoring(),
        PermissionKind::ScreenRecording => {
            let result = permissions::request_screen_recording();
            if !result {
                open_permission_settings(kind);
            }
            result
        }
        PermissionKind::FullDiskAccess => permissions::request_full_disk_access(),
    }
}

pub(super) fn permission_instruction_text(
    kind: PermissionKind,
    status: PermissionUiStatus,
    requested: bool,
) -> Option<&'static str> {
    match kind.recovery_strategy() {
        PermissionRecoveryStrategy::AppRestartRequired => {
            if should_wait_for_restart(kind, status, requested) {
                Some(
                    "Permission granted. Restart CodeScribe to activate it. On relaunch onboarding will resume here automatically.",
                )
            } else if status == PermissionUiStatus::Granted {
                None
            } else if kind == PermissionKind::FullDiskAccess {
                Some(
                    "After enabling CodeScribe in System Settings > Privacy & Security > Full Disk Access, restart CodeScribe. On relaunch onboarding will resume here automatically.",
                )
            } else {
                Some(
                    "Enable this in System Settings, then restart CodeScribe. On relaunch onboarding will resume here automatically.",
                )
            }
        }
        PermissionRecoveryStrategy::LiveReinitialize => {
            if status == PermissionUiStatus::Denied {
                Some(
                    "Enable this in System Settings. CodeScribe will reconnect global hotkeys live once Accessibility and Input Monitoring are both granted.",
                )
            } else {
                None
            }
        }
        PermissionRecoveryStrategy::LiveRecheck => {
            if status == PermissionUiStatus::Denied {
                Some(
                    "This permission is required to continue onboarding. Enable it in System Settings, then click Try Again. CodeScribe rechecks microphone access live.",
                )
            } else {
                None
            }
        }
    }
}

pub(super) fn permission_status_text(
    kind: PermissionKind,
    status: PermissionUiStatus,
    requested: bool,
) -> &'static str {
    match (kind, status, requested) {
        (_, PermissionUiStatus::Granted, true)
            if kind.recovery_strategy() == PermissionRecoveryStrategy::AppRestartRequired =>
        {
            STATUS_RESTART_REQUIRED
        }
        (_, PermissionUiStatus::NotDetermined, _) => STATUS_NOT_DETERMINED,
        (_, PermissionUiStatus::Granted, _) => STATUS_GRANTED,
        (_, PermissionUiStatus::Denied, _) => STATUS_DENIED,
    }
}

pub(super) fn permission_status_color(status: PermissionUiStatus) -> Id {
    match status {
        PermissionUiStatus::NotDetermined => system_secondary_color(),
        PermissionUiStatus::Granted => system_green_color(),
        PermissionUiStatus::Denied => system_red_color(),
    }
}
