//! Action handlers for voice chat overlay
//!
//! Contains Objective-C class registration and action handler functions.

use objc::declare::ClassDecl;
use objc::runtime::{Class, Object, Sel};
use objc::{msg_send, sel, sel_impl};
use std::sync::Once;
use tracing::{debug, info};

use crate::ui::bootstrap;
use crate::ui_helpers::{get_text_field_string, ns_string, set_hidden, set_text_field_string};

use super::api::{
    clear_overlay_state, clear_voice_chat_text_impl, commit_last_user_message_impl,
    discard_last_message_impl, filter_drawer, handle_card_copy, handle_card_delete,
    handle_card_edit, handle_card_favorite, send_draft_message_impl, update_active_tab_impl,
};
use super::state::{ChatRole, OVERLAY_STATE, Tab};

// Type alias for Objective-C object pointers
pub type Id = *mut Object;

static ACTION_HANDLER_INIT: Once = Once::new();
static mut ACTION_HANDLER_CLASS: *const Class = std::ptr::null();
static WINDOW_DELEGATE_INIT: Once = Once::new();
static mut WINDOW_DELEGATE_CLASS: *const Class = std::ptr::null();

/// Get or create the action handler class for UI controls
pub fn action_handler_class() -> *const Class {
    unsafe {
        ACTION_HANDLER_INIT.call_once(|| {
            let superclass = Class::get("NSObject").expect("NSObject not found");
            let mut decl = ClassDecl::new("VoiceChatOverlayActionHandler", superclass)
                .expect("Failed to declare handler class");
            decl.add_method(sel!(onSend:), on_send as extern "C" fn(&Object, Sel, Id));
            decl.add_method(
                sel!(onInputSubmit:),
                on_send as extern "C" fn(&Object, Sel, Id),
            );
            decl.add_method(
                sel!(onTabChanged:),
                on_tab_changed as extern "C" fn(&Object, Sel, Id),
            );
            decl.add_method(sel!(onClose:), on_close as extern "C" fn(&Object, Sel, Id));
            decl.add_method(
                sel!(onCopyLastResponse:),
                on_copy_last_response as extern "C" fn(&Object, Sel, Id),
            );
            decl.add_method(
                sel!(onCopyMessage:),
                on_copy_message as extern "C" fn(&Object, Sel, Id),
            );
            decl.add_method(
                sel!(onCardCopy:),
                on_card_copy as extern "C" fn(&Object, Sel, Id),
            );
            decl.add_method(
                sel!(onCardEdit:),
                on_card_edit as extern "C" fn(&Object, Sel, Id),
            );
            decl.add_method(
                sel!(onCardDelete:),
                on_card_delete as extern "C" fn(&Object, Sel, Id),
            );
            decl.add_method(
                sel!(onCardFavorite:),
                on_card_favorite as extern "C" fn(&Object, Sel, Id),
            );
            decl.add_method(
                sel!(onSearchChanged:),
                on_search_changed as extern "C" fn(&Object, Sel, Id),
            );
            decl.add_method(
                sel!(onNewThread:),
                on_new_thread as extern "C" fn(&Object, Sel, Id),
            );
            decl.add_method(
                sel!(onCommitMessage:),
                on_commit_message as extern "C" fn(&Object, Sel, Id),
            );
            decl.add_method(
                sel!(onDiscardMessage:),
                on_discard_message as extern "C" fn(&Object, Sel, Id),
            );
            let cls = decl.register();
            ACTION_HANDLER_CLASS = cls;
        });
        ACTION_HANDLER_CLASS
    }
}

/// Get or create the window delegate class
pub fn window_delegate_class() -> *const Class {
    unsafe {
        WINDOW_DELEGATE_INIT.call_once(|| {
            let superclass = Class::get("NSObject").expect("NSObject not found");
            let mut decl = ClassDecl::new("VoiceChatOverlayWindowDelegate", superclass)
                .expect("Failed to declare window delegate class");
            decl.add_method(
                sel!(windowWillClose:),
                on_window_will_close as extern "C" fn(&Object, Sel, Id),
            );
            let cls = decl.register();
            WINDOW_DELEGATE_CLASS = cls;
        });
        WINDOW_DELEGATE_CLASS
    }
}

// ═══════════════════════════════════════════════════════════
// Action Handlers
// ═══════════════════════════════════════════════════════════

extern "C" fn on_send(_this: &Object, _cmd: Sel, _sender: Id) {
    send_draft_message_impl();
}

extern "C" fn on_close(_this: &Object, _cmd: Sel, _sender: Id) {
    super::api::hide_voice_chat_overlay();
    if bootstrap::should_show_bootstrap() {
        bootstrap::handle_hotkey_done();
    }
}

extern "C" fn on_window_will_close(_this: &Object, _cmd: Sel, _notification: Id) {
    let mut state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
    clear_overlay_state(&mut state);
    debug!("Voice chat overlay closed by user");
}

extern "C" fn on_tab_changed(_this: &Object, _cmd: Sel, sender: Id) {
    unsafe {
        let selected: isize = msg_send![sender, selectedSegment];
        let tab = if selected == 0 {
            Tab::Drawer
        } else {
            Tab::Agent
        };
        update_active_tab_impl(tab);
        info!("Tab changed to: {:?}", tab);
    }
}

extern "C" fn on_copy_last_response(_this: &Object, _cmd: Sel, _sender: Id) {
    let state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(msg) = state
        .messages
        .iter()
        .rev()
        .find(|m| m.role == ChatRole::Assistant)
    {
        copy_to_clipboard(&msg.text);
        info!("Copied last assistant response to clipboard");
    } else {
        info!("No assistant response to copy");
    }
}

extern "C" fn on_copy_message(_this: &Object, _cmd: Sel, sender: Id) {
    let index: isize = unsafe { msg_send![sender, tag] };
    let index = index.max(0) as usize;
    let state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(message) = state.messages.get(index) {
        copy_to_clipboard(&message.text);
    }
}

extern "C" fn on_card_copy(_this: &Object, _cmd: Sel, sender: Id) {
    let index: isize = unsafe { msg_send![sender, tag] };
    let index = index.max(0) as usize;
    handle_card_copy(index);
}

extern "C" fn on_card_edit(_this: &Object, _cmd: Sel, sender: Id) {
    let index: isize = unsafe { msg_send![sender, tag] };
    let index = index.max(0) as usize;
    handle_card_edit(index);
}

extern "C" fn on_card_delete(_this: &Object, _cmd: Sel, sender: Id) {
    let index: isize = unsafe { msg_send![sender, tag] };
    let index = index.max(0) as usize;
    handle_card_delete(index);
}

extern "C" fn on_card_favorite(_this: &Object, _cmd: Sel, sender: Id) {
    let index: isize = unsafe { msg_send![sender, tag] };
    let index = index.max(0) as usize;
    handle_card_favorite(index);
}

extern "C" fn on_search_changed(_this: &Object, _cmd: Sel, sender: Id) {
    let query = unsafe { get_text_field_string(sender) };
    filter_drawer(&query);
}

extern "C" fn on_new_thread(_this: &Object, _cmd: Sel, _sender: Id) {
    clear_voice_chat_text_impl();
    info!("New thread started");
}

extern "C" fn on_commit_message(_this: &Object, _cmd: Sel, _sender: Id) {
    commit_last_user_message_impl();
    info!("Draft message committed");
}

extern "C" fn on_discard_message(_this: &Object, _cmd: Sel, _sender: Id) {
    discard_last_message_impl();
    info!("Draft message discarded");
}

// ═══════════════════════════════════════════════════════════
// Utilities
// ═══════════════════════════════════════════════════════════

pub fn copy_to_clipboard(text: &str) {
    unsafe {
        let ns_pasteboard = Class::get("NSPasteboard").unwrap();
        let pasteboard: Id = msg_send![ns_pasteboard, generalPasteboard];
        let _: () = msg_send![pasteboard, clearContents];

        let ns_array = Class::get("NSArray").unwrap();
        let ns_string_class = Class::get("NSString").unwrap();

        let text_str = ns_string(text);
        let array: Id = msg_send![ns_array, arrayWithObject: text_str];
        let _: () = msg_send![pasteboard, writeObjects: array];
        let _: Id =
            msg_send![ns_string_class, stringWithUTF8String: c"NSStringPboardType".as_ptr()];
    }
}

pub fn clear_search_field() {
    let state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(search_field) = state.search_field {
        unsafe {
            set_text_field_string(search_field as Id, "");
        }
        unsafe {
            set_hidden(search_field as Id, false);
        }
    }
}
