//! Mode shortcut binding recorder: capture, validation, and label refresh.

use super::*;

#[derive(Default)]
pub(super) struct ModeBindingRecorderState {
    monitor_installed: bool,
    target_mode: Option<WorkMode>,
}

lazy_static! {
    static ref MODE_BINDING_RECORDER_STATE: Mutex<ModeBindingRecorderState> =
        Mutex::new(ModeBindingRecorderState::default());
}

pub(super) fn mode_from_tag(tag: isize) -> Option<WorkMode> {
    match tag {
        MODE_DICTATION_TAG => Some(WorkMode::Dictation),
        MODE_FORMATTING_TAG => Some(WorkMode::Formatting),
        MODE_ASSISTIVE_TAG => Some(WorkMode::Assistive),
        _ => None,
    }
}

pub(super) fn mode_from_disable_tag(tag: isize) -> Option<WorkMode> {
    mode_from_tag(tag - MODE_DISABLE_TAG_OFFSET)
}

pub(super) fn mode_from_double_ctrl_tag(tag: isize) -> bool {
    tag == MODE_DICTATION_DOUBLE_CTRL_TAG
}

pub(super) fn mode_label_slot(mode: WorkMode) -> usize {
    match mode {
        WorkMode::Dictation => 0,
        WorkMode::Formatting => 1,
        WorkMode::Assistive => 2,
    }
}

pub(super) fn set_mode_recorder_hint(text: &str, is_error: bool) {
    let hint_ptr = {
        let state = SETTINGS_WINDOW_STATE
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        state.keys_recorder_hint_label
    };
    let Some(hint_ptr) = hint_ptr else {
        return;
    };
    // SAFETY: see module-level # Safety doc — main-thread AppKit / msg_send! access on retained `Id` pointers.
    unsafe {
        let hint_label = hint_ptr as Id;
        set_text_field_string(hint_label, text);
        let color = if is_error {
            ui_colors::bubble_error_text()
        } else {
            crate::ui_helpers::color_secondary_label()
        };
        let _: () = msg_send![hint_label, setTextColor: color];
    }
}

pub(super) fn refresh_mode_binding_labels() {
    let settings = UserSettings::load();
    let state = SETTINGS_WINDOW_STATE
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    for mode in [
        WorkMode::Dictation,
        WorkMode::Formatting,
        WorkMode::Assistive,
    ] {
        if let Some(label_ptr) = state.keys_mode_binding_labels[mode_label_slot(mode)] {
            let text = settings.mode_binding_for(mode).label().to_string();
            // SAFETY: see module-level # Safety doc — main-thread AppKit / msg_send! access on retained `Id` pointers.
            unsafe {
                set_text_field_string(label_ptr as Id, &text);
            }
        }
    }
}

pub(super) fn binding_from_recorded_event(
    mode: WorkMode,
    event_type: u64,
    keycode: u16,
    flags: u64,
) -> Option<ShortcutBinding> {
    // NSEventModifierFlagShift/Control/Option/Command
    const SHIFT: u64 = 1 << 17;
    const CONTROL: u64 = 1 << 18;
    const OPTION: u64 = 1 << 19;
    const COMMAND: u64 = 1 << 20;
    const EVENT_TYPE_FLAGS_CHANGED: u64 = 12;

    match mode {
        WorkMode::Dictation => match keycode {
            63 => Some(ShortcutBinding::HoldFn),
            59 | 62 => {
                if (flags & OPTION) != 0 {
                    Some(ShortcutBinding::HoldCtrlAlt)
                } else if (flags & SHIFT) != 0 {
                    Some(ShortcutBinding::HoldCtrlShift)
                } else if (flags & COMMAND) != 0 {
                    Some(ShortcutBinding::HoldCtrlCmd)
                } else if event_type == EVENT_TYPE_FLAGS_CHANGED && (flags & CONTROL) != 0 {
                    Some(ShortcutBinding::HoldCtrl)
                } else {
                    None
                }
            }
            _ => None,
        },
        WorkMode::Formatting => match keycode {
            58 => Some(ShortcutBinding::DoubleLeftOption),
            _ => None,
        },
        WorkMode::Assistive => match keycode {
            63 => Some(ShortcutBinding::HoldFn),
            59 | 62 => {
                if (flags & OPTION) != 0 {
                    Some(ShortcutBinding::HoldCtrlAlt)
                } else if (flags & SHIFT) != 0 {
                    Some(ShortcutBinding::HoldCtrlShift)
                } else if (flags & COMMAND) != 0 {
                    Some(ShortcutBinding::HoldCtrlCmd)
                } else if event_type == EVENT_TYPE_FLAGS_CHANGED && (flags & CONTROL) != 0 {
                    Some(ShortcutBinding::HoldCtrl)
                } else {
                    None
                }
            }
            61 => Some(ShortcutBinding::DoubleRightOption),
            _ => None,
        },
    }
}

pub(super) fn mode_accepts_binding(mode: WorkMode, binding: ShortcutBinding) -> bool {
    match mode {
        WorkMode::Dictation => matches!(
            binding,
            ShortcutBinding::Disabled
                | ShortcutBinding::HoldFn
                | ShortcutBinding::HoldCtrl
                | ShortcutBinding::HoldCtrlAlt
                | ShortcutBinding::HoldCtrlShift
                | ShortcutBinding::HoldCtrlCmd
                | ShortcutBinding::DoubleCtrl
        ),
        WorkMode::Formatting => {
            matches!(
                binding,
                ShortcutBinding::Disabled | ShortcutBinding::DoubleLeftOption
            )
        }
        WorkMode::Assistive => {
            matches!(
                binding,
                ShortcutBinding::Disabled
                    | ShortcutBinding::HoldFn
                    | ShortcutBinding::HoldCtrl
                    | ShortcutBinding::HoldCtrlAlt
                    | ShortcutBinding::HoldCtrlShift
                    | ShortcutBinding::HoldCtrlCmd
                    | ShortcutBinding::DoubleRightOption
            )
        }
    }
}

pub(super) fn mode_binding_selection_error(
    mode: WorkMode,
    binding: ShortcutBinding,
    settings: &UserSettings,
) -> Option<String> {
    if !mode_accepts_binding(mode, binding) {
        return Some(format!(
            "{} mode supports only {} bindings.",
            mode.label(),
            match mode {
                WorkMode::Dictation => "hold modifiers or Double Ctrl",
                WorkMode::Formatting => "Double Left Option",
                WorkMode::Assistive => "hold modifiers or Double Right Option",
            }
        ));
    }

    if mode != WorkMode::Dictation
        && binding != ShortcutBinding::Disabled
        && settings.mode_binding_for(WorkMode::Dictation) == ShortcutBinding::DoubleCtrl
        && matches!(
            binding,
            ShortcutBinding::DoubleLeftOption | ShortcutBinding::DoubleRightOption
        )
    {
        return Some(
            "Dictation is currently on Double Ctrl. Disable it first to use Option bindings."
                .to_string(),
        );
    }

    None
}

pub(super) fn apply_mode_binding(mode: WorkMode, binding: ShortcutBinding) {
    let mut settings = UserSettings::load();
    if let Some(message) = mode_binding_selection_error(mode, binding, &settings) {
        set_mode_recorder_hint(&message, true);
        return;
    }

    settings.set_mode_binding(mode, binding);

    if mode == WorkMode::Dictation && binding == ShortcutBinding::DoubleCtrl {
        settings.set_mode_binding(WorkMode::Formatting, ShortcutBinding::Disabled);
        settings.set_mode_binding(WorkMode::Assistive, ShortcutBinding::Disabled);
    }

    let config = Config::load();
    hotkeys::apply_hotkey_runtime_config(hotkeys::HotkeyRuntimeConfig::from(&config));
    sync_runtime_config_via_ipc();

    refresh_mode_binding_labels();
    refresh_hotkey_conflict_indicator();
    set_mode_recorder_hint(
        &format!("{} mode -> {}", mode.label(), binding.label()),
        false,
    );
}

pub(super) fn apply_recorded_mode_binding(mode: WorkMode, binding: ShortcutBinding) {
    apply_mode_binding(mode, binding);
}

pub(super) fn recorder_capture_mode() -> Option<WorkMode> {
    let recorder = MODE_BINDING_RECORDER_STATE
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    recorder.target_mode
}

pub(super) fn recorder_clear_target_mode() {
    let mut recorder = MODE_BINDING_RECORDER_STATE
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    recorder.target_mode = None;
}

pub(super) fn handle_mode_binding_recorder_event(event: Id) -> Id {
    let Some(mode) = recorder_capture_mode() else {
        return event;
    };

    // SAFETY: see module-level # Safety doc — main-thread AppKit / msg_send! access on retained `Id` pointers.
    unsafe {
        let event_type: u64 = msg_send![event, type];
        let keycode: u16 = msg_send![event, keyCode];
        let flags: u64 = msg_send![event, modifierFlags];

        // Escape cancels recording.
        if event_type == 10 && keycode == 53 {
            recorder_clear_target_mode();
            set_mode_recorder_hint("Mode binding capture cancelled.", false);
            return std::ptr::null_mut();
        }

        if let Some(binding) = binding_from_recorded_event(mode, event_type, keycode, flags) {
            recorder_clear_target_mode();
            apply_recorded_mode_binding(mode, binding);
            return std::ptr::null_mut();
        }
    }

    set_mode_recorder_hint(
        "Unsupported shortcut for this mode. Press Esc to cancel capture.",
        true,
    );
    std::ptr::null_mut()
}

pub(super) fn ensure_mode_binding_recorder_monitor() -> bool {
    let should_install = {
        let recorder = MODE_BINDING_RECORDER_STATE
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        !recorder.monitor_installed
    };
    if !should_install {
        return true;
    }

    // SAFETY: see module-level # Safety doc — main-thread AppKit / msg_send! access on retained `Id` pointers.
    unsafe {
        let ns_event = objc_class("NSEvent");
        let mask: u64 = (1_u64 << 10) | (1_u64 << 12); // keyDown + flagsChanged
        let handler: block2::RcBlock<
            dyn Fn(*mut objc2::runtime::AnyObject) -> *mut objc2::runtime::AnyObject,
        > = block2::RcBlock::new(|event: *mut objc2::runtime::AnyObject| {
            handle_mode_binding_recorder_event(event.cast()).cast()
        });
        let monitor: Id =
            msg_send![ns_event, addLocalMonitorForEventsMatchingMask: mask handler: &*handler];
        if monitor.is_null() {
            warn!("Mode binding recorder: failed to install local event monitor");
            return false;
        }
        let _ = block2::RcBlock::into_raw(handler);
    }

    let mut recorder = MODE_BINDING_RECORDER_STATE
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    recorder.monitor_installed = true;
    true
}

pub(super) fn start_mode_binding_recorder(mode: WorkMode) {
    if !ensure_mode_binding_recorder_monitor() {
        set_mode_recorder_hint("Mode binding recorder failed to initialize.", true);
        return;
    }
    {
        let mut recorder = MODE_BINDING_RECORDER_STATE
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        recorder.target_mode = Some(mode);
    }
    set_mode_recorder_hint(
        &format!(
            "Recording {} binding... Press Fn/Ctrl/Option (Esc to cancel).",
            mode.label()
        ),
        false,
    );
}
