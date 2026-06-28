//! Hotkey conflict detection: status indicator and details sheet.

use super::*;

pub(super) fn hotkey_conflicts(_config: &Config) -> Vec<shortcut_registry::HotkeyConflict> {
    let settings = UserSettings::load();
    shortcut_registry::detect_hotkey_conflicts(&settings)
}

pub(super) fn hotkey_conflict_status_from(
    conflicts: &[shortcut_registry::HotkeyConflict],
) -> (String, bool) {
    if conflicts.is_empty() {
        return ("Mode shortcuts: clear.".to_string(), false);
    }

    let first = &conflicts[0];
    let extra = conflicts.len().saturating_sub(1);
    let suffix = if extra > 0 {
        format!(" (+{} more)", extra)
    } else {
        String::new()
    };

    (
        format!(
            "Review shortcut: {} -> {}{}",
            first.gesture.label(),
            first.message,
            suffix
        ),
        true,
    )
}

pub(super) fn hotkey_conflict_status(config: &Config) -> (String, bool) {
    let conflicts = hotkey_conflicts(config);
    hotkey_conflict_status_from(&conflicts)
}

pub(super) fn hotkey_conflict_details_text(
    conflicts: &[shortcut_registry::HotkeyConflict],
) -> String {
    if conflicts.is_empty() {
        return "No conflicts detected in current mode shortcuts.".to_string();
    }

    let mut lines = vec![
        "Codescribe detected shortcuts that may overlap current mode bindings:".to_string(),
        String::new(),
    ];
    for (index, conflict) in conflicts.iter().enumerate() {
        lines.push(format!(
            "{}. {} -> {}",
            index + 1,
            conflict.gesture.label(),
            conflict.message
        ));
    }
    lines.push(String::new());
    lines.push("Recommendation: change that mode binding only if the gesture does not behave correctly at runtime.".to_string());
    lines.join("\n")
}

pub(super) fn set_hotkey_conflict_details_button_enabled(button_ptr: Option<usize>, enabled: bool) {
    let Some(button_ptr) = button_ptr else {
        return;
    };
    // SAFETY: see module-level # Safety doc — main-thread AppKit / msg_send! access on retained `Id` pointers.
    unsafe {
        let button = button_ptr as Id;
        let _: () = msg_send![button, setEnabled: enabled];
    }
}

pub(super) fn refresh_hotkey_conflict_indicator() {
    let config = Config::load();
    let (label_ptr, button_ptr) = {
        let state = SETTINGS_WINDOW_STATE
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        (
            state.keys_conflict_label,
            state.keys_conflict_details_button,
        )
    };
    apply_hotkey_conflict_indicator(label_ptr, button_ptr, &config);
}

pub(super) fn apply_hotkey_conflict_indicator(
    label_ptr: Option<usize>,
    button_ptr: Option<usize>,
    config: &Config,
) {
    let conflicts = hotkey_conflicts(config);
    let (text, has_conflict) = hotkey_conflict_status_from(&conflicts);
    set_hotkey_conflict_details_button_enabled(button_ptr, has_conflict);

    let Some(label_ptr) = label_ptr else {
        return;
    };
    // SAFETY: see module-level # Safety doc — main-thread AppKit / msg_send! access on retained `Id` pointers.
    unsafe {
        let label = label_ptr as Id;
        set_text_field_string(label, &text);
        let color = if has_conflict {
            ui_colors::bubble_error_text()
        } else {
            crate::ui_helpers::color_secondary_label()
        };
        let _: () = msg_send![label, setTextColor: color];
    }
}

pub(super) fn show_hotkey_conflicts_sheet() {
    let config = Config::load();
    let conflicts = hotkey_conflicts(&config);
    let title = if conflicts.is_empty() {
        "No Shortcut Conflicts"
    } else {
        "Shortcut Conflicts Detected"
    };
    let details = hotkey_conflict_details_text(&conflicts);
    let window_ptr = {
        let state = SETTINGS_WINDOW_STATE
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        state.window
    };

    // SAFETY: see module-level # Safety doc — main-thread AppKit / msg_send! access on retained `Id` pointers.
    unsafe {
        let ns_alert = objc_class("NSAlert");
        let alert: Id = msg_send![ns_alert, new];
        let _: () = msg_send![alert, setMessageText: ns_string(title)];
        let _: () = msg_send![alert, setInformativeText: ns_string(&details)];
        let _: () = msg_send![alert, setAlertStyle: 1_isize]; // NSAlertStyleInformational
        let _: () = msg_send![alert, addButtonWithTitle: ns_string("OK")];

        if let Some(window_ptr) = window_ptr {
            let window = window_ptr as Id;
            if !window.is_null() {
                let nil: Id = std::ptr::null_mut();
                let _: () =
                    msg_send![alert, beginSheetModalForWindow: window completionHandler: nil];
                return;
            }
        }

        let _: isize = msg_send![alert, runModal];
    }
}
