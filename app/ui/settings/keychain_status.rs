//! Keychain-backed API key status indicators.

use super::*;

pub(super) fn keychain_key_is_set(account: &str) -> bool {
    std::env::var(account)
        .ok()
        .map(|v| !v.trim().is_empty())
        .unwrap_or(false)
}

pub(super) fn key_status_text(is_set: bool) -> &'static str {
    if is_set {
        "Stored in Keychain"
    } else {
        "Not set"
    }
}

pub(super) fn key_status_color(is_set: bool) -> Id {
    if is_set {
        ui_colors::status_granted()
    } else {
        ui_colors::secondary_label()
    }
}

pub(super) fn key_status_symbol_name(is_set: bool) -> &'static str {
    if is_set {
        "checkmark.seal.fill"
    } else {
        "circle"
    }
}

pub(super) fn formatting_key_is_set() -> bool {
    keychain_key_is_set("LLM_FORMATTING_API_KEY")
}

pub(super) unsafe fn update_key_status_indicator(indicator: Id, is_set: bool) {
    let _ =
        // SAFETY: see module-level # Safety doc — main-thread AppKit / msg_send! access on retained `Id` pointers.
        unsafe { crate::ui_helpers::set_button_symbol(indicator, key_status_symbol_name(is_set)) };
    let supports_tint: bool = msg_send![indicator, respondsToSelector: sel!(setContentTintColor:)];
    if supports_tint {
        let _: () = msg_send![indicator, setContentTintColor: key_status_color(is_set)];
    }
}

pub(super) unsafe fn create_key_status_indicator(frame: CGRect, is_set: bool) -> Id {
    let ns_button = objc_class("NSButton");
    let indicator: Id = msg_send![ns_button, alloc];
    let indicator: Id = msg_send![indicator, initWithFrame: frame];
    let _: () = msg_send![indicator, setBordered: false];
    let _: () = msg_send![indicator, setEnabled: false];
    let _: () = msg_send![indicator, setTitle: ns_string("")];
    // SAFETY: see module-level # Safety doc — main-thread AppKit / msg_send! access on retained `Id` pointers.
    unsafe {
        update_key_status_indicator(indicator, is_set);
    }
    indicator
}

pub(super) fn update_keychain_status_labels() {
    let (llm_icon, llm_label, assist_icon, assist_label) = {
        let state = SETTINGS_WINDOW_STATE
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        (
            state.llm_key_status_icon,
            state.llm_key_status_label,
            state.assistive_key_status_icon,
            state.assistive_key_status_label,
        )
    };
    // SAFETY: see module-level # Safety doc — main-thread AppKit / msg_send! access on retained `Id` pointers.
    unsafe {
        if let Some(ptr) = llm_icon {
            let is_set = formatting_key_is_set();
            update_key_status_indicator(ptr as Id, is_set);
        }
        if let Some(ptr) = llm_label {
            let is_set = formatting_key_is_set();
            let label = ptr as Id;
            set_text_field_string(label, key_status_text(is_set));
            let _: () = msg_send![label, setTextColor: key_status_color(is_set)];
        }
        if let Some(ptr) = assist_icon {
            let is_set = keychain_key_is_set("LLM_ASSISTIVE_API_KEY");
            update_key_status_indicator(ptr as Id, is_set);
        }
        if let Some(ptr) = assist_label {
            let is_set = keychain_key_is_set("LLM_ASSISTIVE_API_KEY");
            let label = ptr as Id;
            set_text_field_string(label, key_status_text(is_set));
            let _: () = msg_send![label, setTextColor: key_status_color(is_set)];
        }
    }
}

pub(super) fn clear_keychain_entry(account: &str, field_ptr: Option<usize>) {
    if let Err(e) = keychain::delete_key(account) {
        warn!("Failed to delete {account} from Keychain: {e}");
    } else {
        info!("Deleted {account} from Keychain");
    }
    // SAFETY: see module-level # Safety doc — main-thread AppKit / msg_send! access on retained `Id` pointers.
    unsafe { std::env::remove_var(account) };
    if let Some(ptr) = field_ptr {
        // SAFETY: see module-level # Safety doc — main-thread AppKit / msg_send! access on retained `Id` pointers.
        unsafe { set_text_field_string(ptr as Id, "") };
    }
    update_keychain_status_labels();
}
