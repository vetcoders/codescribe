//! Action handlers for voice chat overlay
//!
//! Contains Objective-C class registration and action handler functions.

use objc::declare::ClassDecl;
use objc::runtime::{Class, Object, Sel};
use objc::{msg_send, sel, sel_impl};
use std::sync::Once;
use tracing::{debug, info, warn};

use crate::ui_helpers::{animate_fade, get_text, window_close};

use super::api::{
    clear_overlay_state, refresh_drawer, send_draft_message_impl, set_active_tab_impl,
};
use super::state::{ChatRole, OVERLAY_STATE, Tab};

// Type alias for Objective-C object pointers
// SAFETY: raw Objective-C pointers used in AppKit FFI.
type Id = *mut Object;

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
                sel!(onCopyMessage:),
                on_copy_message as extern "C" fn(&Object, Sel, Id),
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
    let mut state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(window_ptr) = state.window {
        unsafe {
            let window = window_ptr as Id;
            animate_fade(window, 0.0, 0.15);
            window_close(window);
        }
    }
    clear_overlay_state(&mut state);
    debug!("Voice chat overlay closed by user");
}

extern "C" fn on_window_will_close(_this: &Object, _cmd: Sel, _notification: Id) {
    // Window is closing (user clicked close). Clear state to avoid use-after-free.
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
        set_active_tab_impl(tab);
    }
}

extern "C" fn on_copy_last_response(_this: &Object, _cmd: Sel, _sender: Id) {
    let state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
    // Find last assistant message
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

extern "C" fn on_card_copy(_this: &Object, _cmd: Sel, sender: Id) {
    unsafe {
        let tag: isize = msg_send![sender, tag];
        let mut state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
        let Some(entry) = state.drawer_entries.get(tag as usize) else {
            warn!("Invalid drawer entry index for copy: {}", tag);
            return;
        };
        match std::fs::read_to_string(&entry.path) {
            Ok(content) => {
                copy_to_clipboard(&content);
                info!("Copied drawer entry to clipboard: {}", entry.path.display());
            }
            Err(err) => {
                warn!("Failed to read entry {}: {}", entry.path.display(), err);
            }
        }
    }
}

extern "C" fn on_card_edit(_this: &Object, _cmd: Sel, sender: Id) {
    unsafe {
        let tag: isize = msg_send![sender, tag];
        let state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
        let Some(entry) = state.drawer_entries.get(tag as usize) else {
            warn!("Invalid drawer entry index for edit: {}", tag);
            return;
        };
        let opened = crate::ui_helpers::open_file_in_editor(&entry.path);
        info!(
            "Opened drawer entry in editor: {} (ok={})",
            entry.path.display(),
            opened
        );
    }
}

extern "C" fn on_card_delete(_this: &Object, _cmd: Sel, sender: Id) {
    unsafe {
        let tag: isize = msg_send![sender, tag];
        let mut state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
        let Some(entry) = state.drawer_entries.get(tag as usize) else {
            warn!("Invalid drawer entry index for delete: {}", tag);
            return;
        };
        match std::fs::remove_file(&entry.path) {
            Ok(()) => {
                info!("Deleted drawer entry: {}", entry.path.display());
            }
            Err(err) => {
                warn!("Failed to delete entry {}: {}", entry.path.display(), err);
            }
        }
    }
    refresh_drawer();
}

extern "C" fn on_card_favorite(_this: &Object, _cmd: Sel, sender: Id) {
    unsafe {
        let tag: isize = msg_send![sender, tag];
        let mut state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(entry) = state.drawer_entries.get_mut(tag as usize) {
            entry.is_favorite = !entry.is_favorite;
            info!(
                "Toggled favorite for {}: {}",
                entry.path.display(),
                entry.is_favorite
            );
        }
    }
    refresh_drawer();
}

extern "C" fn on_search_changed(_this: &Object, _cmd: Sel, sender: Id) {
    unsafe {
        let query = get_text(sender);
        super::api::filter_drawer(&query);
    }
}

/// Copy a specific message by index (retrieved from button tag)
extern "C" fn on_copy_message(_this: &Object, _cmd: Sel, sender: Id) {
    unsafe {
        // Get message index from button's tag
        let tag: isize = msg_send![sender, tag];
        let msg_index = tag as usize;

        let state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(message) = state.messages.get(msg_index) {
            copy_to_clipboard(&message.text);
            debug!("Copied message {} to clipboard", msg_index);
        } else {
            debug!("Invalid message index: {}", msg_index);
        }
    }
}

// ═══════════════════════════════════════════════════════════
// Helper Functions
// ═══════════════════════════════════════════════════════════

/// Copy text to system clipboard
pub fn copy_to_clipboard(text: &str) {
    unsafe {
        let pasteboard_class = Class::get("NSPasteboard").unwrap();
        let pasteboard: Id = msg_send![pasteboard_class, generalPasteboard];
        let _: () = msg_send![pasteboard, clearContents];

        let ns_string_class = Class::get("NSString").unwrap();
        let mut c_str = text.as_bytes().to_vec();
        c_str.push(0);
        let ns_str: Id = msg_send![ns_string_class, stringWithUTF8String: c_str.as_ptr()];

        // NSPasteboardTypeString = "public.utf8-plain-text"
        let type_str: Id =
            msg_send![ns_string_class, stringWithUTF8String: c"public.utf8-plain-text".as_ptr()];
        let _: () = msg_send![pasteboard, setString: ns_str forType: type_str];
    }
}
