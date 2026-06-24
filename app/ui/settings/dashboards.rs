//! Quality and diagnostics dashboard refresh.

use super::*;

pub(super) fn refresh_quality_dashboard() {
    Queue::main().exec_async(move || unsafe {
        let (available_label, pending_label, last_check_label, report_label, open_report_button) = {
            let state = SETTINGS_WINDOW_STATE
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            (
                state.quality_available_label,
                state.quality_pending_label,
                state.quality_last_check_label,
                state.qube_report_label,
                state.quality_open_report_button,
            )
        };

        let snapshot = crate::qube_lifecycle::dashboard_snapshot();
        let daemon_state = &snapshot.daemon_state;

        if let Some(ptr) = available_label {
            let label = ptr as Id;
            set_text_field_string(label, snapshot.availability_label());
            let _: () = msg_send![
                label,
                setTextColor: if snapshot.available {
                    ui_colors::status_granted()
                } else {
                    ui_colors::status_warning()
                }
            ];
        }

        if let Some(ptr) = pending_label {
            let label = ptr as Id;
            set_text_field_string(label, &daemon_state.pending_mismatches.to_string());
            let _: () = msg_send![
                label,
                setTextColor: if daemon_state.pending_mismatches > 0 {
                    ui_colors::status_warning()
                } else {
                    crate::ui_helpers::color_secondary_label()
                }
            ];
        }

        if let Some(ptr) = last_check_label {
            set_text_field_string(
                ptr as Id,
                &quality_last_check_text(&daemon_state.last_check),
            );
        }

        if let Some(ptr) = report_label {
            set_text_field_string(ptr as Id, &qube_report_text(daemon_state));
        }

        if let Some(ptr) = open_report_button {
            let _: () = msg_send![ptr as Id, setEnabled: qube_report_exists(daemon_state)];
        }
    });
}

pub(super) fn refresh_diagnostics_dashboard() {
    Queue::main().exec_async(move || unsafe {
        let (permission_labels, conflict_label, conflict_button, status_label) = {
            let state = SETTINGS_WINDOW_STATE
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            (
                state.diagnostics_permission_labels,
                state.diagnostics_conflict_label,
                state.diagnostics_conflict_details_button,
                state.diagnostics_status_label,
            )
        };

        for kind in PERMISSION_ORDER {
            let idx = kind.index();
            let status = permission_status(kind);
            if let Some(ptr) = permission_labels[idx] {
                let label = ptr as Id;
                set_text_field_string(label, permission_status_text(status));
                let _: () = msg_send![label, setTextColor: permission_status_color(status)];
            }
        }

        let config = Config::load();
        apply_hotkey_conflict_indicator(conflict_label, conflict_button, &config);

        if let Some(ptr) = status_label {
            set_text_field_string(
                ptr as Id,
                "Use Copy diagnostics to capture a full environment + permission report.",
            );
        }
    });
}

// ============================================================================
// Modes & Shortcuts tab
// ============================================================================

pub(super) fn quality_last_check_text(last_check: &str) -> String {
    let trimmed = last_check.trim();
    if trimmed.is_empty() {
        "Never".to_string()
    } else {
        trimmed.to_string()
    }
}

pub(super) fn qube_report_exists(state: &crate::qube_daemon::QubeDaemonState) -> bool {
    state
        .latest_report
        .as_ref()
        .map(|dir| PathBuf::from(dir).join("index.html").exists())
        .unwrap_or(false)
}

pub(super) fn qube_report_text(state: &crate::qube_daemon::QubeDaemonState) -> String {
    match state.latest_report.as_ref() {
        Some(dir) => {
            let html_path = PathBuf::from(dir).join("index.html");
            if html_path.exists() {
                html_path.display().to_string()
            } else {
                format!("{dir} (missing index.html)")
            }
        }
        None => "(none)".to_string(),
    }
}
