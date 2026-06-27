//! Onboarding-local AppKit widget glue: radio buttons, label configuration,
//! optional-pointer setters, radio group sync, and system color lookups.

use core_graphics::geometry::CGRect;
use objc::runtime::Class;
use objc::{msg_send, sel, sel_impl};

use crate::ui::shared::helpers::{ns_string, set_hidden, set_text_field_string};

use super::Id;
use super::state::{HotkeyModeChoice, LanguageChoice, OnboardingModeChoice, UiRefs};

pub(super) fn configure_label(label: Id, centered: bool, multiline: bool) {
    unsafe {
        // NSTextAlignment: 0=left, 1=center, 2=right
        let align = if centered { 1_isize } else { 0_isize };
        let _: () = msg_send![label, setAlignment: align];

        if multiline {
            let _: () = msg_send![label, setUsesSingleLineMode: false];
            let _: () = msg_send![label, setLineBreakMode: 0_isize];
            let cell: Id = msg_send![label, cell];
            if !cell.is_null() {
                let _: () = msg_send![cell, setWraps: true];
                let _: () = msg_send![cell, setScrollable: false];
                let _: () = msg_send![cell, setLineBreakMode: 0_isize];
            }
        }
    }
}

pub(super) fn create_radio_button(frame: CGRect, title: &str, selected: bool) -> Id {
    unsafe {
        let ns_button = Class::get("NSButton").unwrap();
        let button: Id = msg_send![ns_button, alloc];
        let button: Id = msg_send![button, initWithFrame: frame];
        let _: () = msg_send![button, setButtonType: 4_isize]; // NSRadioButton
        let _: () = msg_send![button, setTitle: ns_string(title)];
        let _: () = msg_send![button, setState: if selected { 1_isize } else { 0_isize }];
        button
    }
}

pub(super) fn set_text_if_present(ptr: Option<usize>, text: &str) {
    unsafe {
        if let Some(value) = ptr {
            set_text_field_string(value as Id, text);
        }
    }
}

pub(super) fn set_button_title_if_present(ptr: Option<usize>, title: &str) {
    unsafe {
        if let Some(value) = ptr {
            let _: () = msg_send![value as Id, setTitle: ns_string(title)];
        }
    }
}

pub(super) fn set_hidden_if_present(ptr: Option<usize>, hidden: bool) {
    unsafe {
        if let Some(value) = ptr {
            set_hidden(value as Id, hidden);
        }
    }
}

pub(super) fn set_label_color_if_present(ptr: Option<usize>, color: Id) {
    unsafe {
        if let Some(value) = ptr {
            let _: () = msg_send![value as Id, setTextColor: color];
        }
    }
}

pub(super) fn sync_language_radios(ui: UiRefs, language: LanguageChoice) {
    unsafe {
        if let Some(en) = ui.language_en_radio {
            let _: () = msg_send![en as Id, setState: if language == LanguageChoice::English { 1_isize } else { 0_isize }];
        }
        if let Some(pl) = ui.language_pl_radio {
            let _: () = msg_send![pl as Id, setState: if language == LanguageChoice::Polish { 1_isize } else { 0_isize }];
        }
    }
}

pub(super) fn sync_mode_radios(ui: UiRefs, mode: OnboardingModeChoice) {
    unsafe {
        if let Some(basic) = ui.mode_basic_radio {
            let _: () = msg_send![basic as Id, setState: if mode == OnboardingModeChoice::Basic { 1_isize } else { 0_isize }];
        }
        if let Some(agentic) = ui.mode_agentic_radio {
            let _: () = msg_send![agentic as Id, setState: if mode == OnboardingModeChoice::Agentic { 1_isize } else { 0_isize }];
        }
    }
}

pub(super) fn sync_hotkey_radios(ui: UiRefs, mode: HotkeyModeChoice) {
    unsafe {
        if let Some(hold) = ui.hotkey_hold_radio {
            let _: () = msg_send![hold as Id, setState: if mode == HotkeyModeChoice::HoldToTalk { 1_isize } else { 0_isize }];
        }
        if let Some(toggle) = ui.hotkey_toggle_radio {
            let _: () = msg_send![toggle as Id, setState: if mode == HotkeyModeChoice::Toggle { 1_isize } else { 0_isize }];
        }
        if let Some(both) = ui.hotkey_both_radio {
            let _: () = msg_send![both as Id, setState: if mode == HotkeyModeChoice::Both { 1_isize } else { 0_isize }];
        }
    }
}

pub(super) fn system_green_color() -> Id {
    unsafe {
        let ns_color = Class::get("NSColor").unwrap();
        msg_send![ns_color, systemGreenColor]
    }
}

pub(super) fn system_red_color() -> Id {
    unsafe {
        let ns_color = Class::get("NSColor").unwrap();
        msg_send![ns_color, systemRedColor]
    }
}

pub(super) fn system_orange_color() -> Id {
    unsafe {
        let ns_color = Class::get("NSColor").unwrap();
        msg_send![ns_color, systemOrangeColor]
    }
}

pub(super) fn system_secondary_color() -> Id {
    unsafe {
        let ns_color = Class::get("NSColor").unwrap();
        msg_send![ns_color, secondaryLabelColor]
    }
}

pub(super) fn get_text_field_string(field: Id) -> String {
    unsafe {
        let value: Id = msg_send![field, stringValue];
        let c_str: *const std::ffi::c_char = msg_send![value, UTF8String];
        if c_str.is_null() {
            return String::new();
        }
        std::ffi::CStr::from_ptr(c_str)
            .to_string_lossy()
            .to_string()
    }
}
