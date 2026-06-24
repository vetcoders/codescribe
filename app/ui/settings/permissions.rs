//! Permission rows, grant actions, and permission polling for the settings window.

use super::*;

pub(super) fn permission_color(granted: bool) -> Id {
    if granted {
        ui_colors::status_granted()
    } else {
        ui_colors::status_denied()
    }
}

pub(super) fn permission_row_label(kind: PermissionKind) -> &'static str {
    kind.title()
}

pub(super) fn permission_action_title(
    kind: PermissionKind,
    status: PermissionStatus,
    requested: bool,
) -> Option<&'static str> {
    if status == PermissionStatus::Granted {
        None
    } else if kind == PermissionKind::FullDiskAccess || requested {
        Some("Open Settings")
    } else {
        Some("Grant")
    }
}

pub(super) fn permission_kind_from_tag(tag: isize) -> Option<PermissionKind> {
    if tag < 0 {
        return None;
    }
    PERMISSION_ORDER.get(tag as usize).copied()
}

pub(super) fn open_system_settings_security() {
    let _ = std::process::Command::new("open")
        .arg("x-apple.systempreferences:com.apple.preference.security")
        .spawn();
}

pub(super) fn handle_permission_action(kind: PermissionKind) {
    let idx = kind.index();
    let already_requested = {
        let mut state = SETTINGS_WINDOW_STATE
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let was_requested = state.permission_requested[idx];
        state.permission_requested[idx] = true;
        was_requested
    };

    if kind == PermissionKind::FullDiskAccess || already_requested {
        open_permission_settings(kind);
        refresh_permission_indicators();
        return;
    }

    if kind == PermissionKind::Microphone {
        thread::spawn(move || {
            let _ = request_permission(kind);
            reconcile_permission_runtime_after_grant(kind);
            refresh_permission_indicators();
        });
        refresh_permission_indicators();
        return;
    }

    let granted = request_permission(kind);
    if !granted
        && matches!(
            kind,
            PermissionKind::Accessibility | PermissionKind::InputMonitoring
        )
    {
        open_permission_settings(kind);
    }

    if granted {
        reconcile_permission_runtime_after_grant(kind);
    }

    refresh_permission_indicators();
}

pub(super) fn start_permission_polling() {
    let should_start = {
        let mut state = SETTINGS_WINDOW_STATE
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        if state.permission_polling {
            false
        } else {
            state.permission_polling = true;
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
                let state = SETTINGS_WINDOW_STATE
                    .lock()
                    .unwrap_or_else(|e| e.into_inner());
                state.permission_polling
            };
            if !keep_running {
                break;
            }
            refresh_permission_indicators();
        }
    });
}

pub(super) fn refresh_permission_indicators() {
    Queue::main().exec_async(move || unsafe {
        let (labels, action_buttons, requested) = {
            let state = SETTINGS_WINDOW_STATE
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            (
                state.permission_labels,
                state.permission_action_buttons,
                state.permission_requested,
            )
        };

        for kind in PERMISSION_ORDER {
            let idx = kind.index();
            let status = permission_status(kind);
            let granted = status == PermissionStatus::Granted;
            let marker = if granted { "\u{2713}" } else { "\u{2715}" };
            let text = format!("{marker} {}", permission_row_label(kind));

            if let Some(label_ptr) = labels[idx] {
                let label = label_ptr as Id;
                set_text_field_string(label, &text);
                let color = permission_color(granted);
                let _: () = msg_send![label, setTextColor: color];
            }

            if let Some(button_ptr) = action_buttons[idx] {
                let action_button = button_ptr as Id;
                if let Some(title) = permission_action_title(kind, status, requested[idx]) {
                    let _: () = msg_send![action_button, setHidden: false];
                    let _: () = msg_send![action_button, setTitle: ns_string(title)];
                } else {
                    let _: () = msg_send![action_button, setHidden: true];
                }
            }
        }

        refresh_diagnostics_dashboard();
    });
}

pub(super) fn permission_status_text(status: PermissionStatus) -> &'static str {
    match status {
        PermissionStatus::Granted => "Granted",
        PermissionStatus::Denied => "Denied",
        PermissionStatus::NotDetermined => "Not determined",
    }
}

pub(super) fn permission_status_color(status: PermissionStatus) -> Id {
    match status {
        PermissionStatus::Granted => ui_colors::status_granted(),
        PermissionStatus::Denied => ui_colors::status_denied(),
        PermissionStatus::NotDetermined => ui_colors::status_warning(),
    }
}
