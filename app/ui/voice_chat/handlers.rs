//! Action handlers for voice chat overlay
//!
//! Contains Objective-C class registration and action handler functions.

use objc::declare::ClassDecl;
use objc::runtime::{Class, Object, Sel};
use objc::{msg_send, sel, sel_impl};
use core_graphics::geometry::{CGPoint, CGRect};
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
                sel!(onAttachMenu:),
                on_attach_menu as extern "C" fn(&Object, Sel, Id),
            );
            decl.add_method(
                sel!(onAttachPick:),
                on_attach_pick as extern "C" fn(&Object, Sel, Id),
            );
            decl.add_method(
                sel!(onAttachClear:),
                on_attach_clear as extern "C" fn(&Object, Sel, Id),
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

extern "C" fn on_attach_menu(this: &Object, _cmd: Sel, sender: Id) {
    unsafe {
        let ns_menu = Class::get("NSMenu").unwrap();
        let ns_menu_item = Class::get("NSMenuItem").unwrap();

        let menu: Id = msg_send![ns_menu, new];
        let target: Id = (this as *const Object) as Id;

        let attach: Id = msg_send![ns_menu_item, alloc];
        let attach: Id = msg_send![
            attach,
            initWithTitle: ns_string("Attach files…")
            action: sel!(onAttachPick:)
            keyEquivalent: ns_string("")
        ];
        let _: () = msg_send![attach, setTarget: target];
        let _: () = msg_send![menu, addItem: attach];

        let count = {
            let state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
            state.attached_files.len()
        };
        if count > 0 {
            let sep: Id = msg_send![ns_menu_item, separatorItem];
            let _: () = msg_send![menu, addItem: sep];

            let clear_title = format!("Clear attachments ({})", count);
            let clear: Id = msg_send![ns_menu_item, alloc];
            let clear: Id = msg_send![
                clear,
                initWithTitle: ns_string(&clear_title)
                action: sel!(onAttachClear:)
                keyEquivalent: ns_string("")
            ];
            let _: () = msg_send![clear, setTarget: target];
            let _: () = msg_send![menu, addItem: clear];
        }

        // Pop up anchored at the button.
        let bounds: CGRect = msg_send![sender, bounds];
        let location = CGPoint::new(0.0, bounds.size.height);
        let nil_item: *mut Object = std::ptr::null_mut();
        let _: bool = msg_send![
            menu,
            popUpMenuPositioningItem: nil_item
            atLocation: location
            inView: sender
        ];
    }
}

extern "C" fn on_attach_pick(_this: &Object, _cmd: Sel, _sender: Id) {
    let picked = crate::ui_helpers::pick_files_open_panel("Attach files");
    if picked.is_empty() {
        return;
    }

    let mut state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
    for p in picked {
        if !state.attached_files.contains(&p) {
            state.attached_files.push(p);
        }
    }
    state.attached_files_last_sent = None;
    update_attach_button_ui_locked(&mut state);
}

extern "C" fn on_attach_clear(_this: &Object, _cmd: Sel, _sender: Id) {
    let mut state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
    state.attached_files.clear();
    state.attached_files_last_sent = None;
    update_attach_button_ui_locked(&mut state);
}

fn update_attach_button_ui_locked(state: &mut super::state::VoiceChatOverlayState) {
    unsafe {
        let Some(btn_ptr) = state.agent_attach_button else {
            return;
        };
        let btn = btn_ptr as Id;
        let count = state.attached_files.len();
        let title = if count == 0 {
            "📎".to_string()
        } else {
            format!("📎{}", count)
        };
        let _: () = msg_send![btn, setTitle: ns_string(&title)];

        if count == 0 {
            let _: () = msg_send![btn, setToolTip: ns_string("Załącz pliki (kontekst dla asystenta)")];
        } else {
            let mut names: Vec<String> = state
                .attached_files
                .iter()
                .filter_map(|p| p.file_name().map(|n| n.to_string_lossy().to_string()))
                .collect();
            names.sort();
            let shown: Vec<String> = names.into_iter().take(3).collect();
            let suffix = if count > 3 { "…" } else { "" };
            let tip = format!("Załączone: {}{}", shown.join(", "), suffix);
            let _: () = msg_send![btn, setToolTip: ns_string(&tip)];
        }
    }
}

extern "C" fn on_close(_this: &Object, _cmd: Sel, _sender: Id) {
    super::api::hide_voice_chat_overlay();
    if bootstrap::should_show_bootstrap() {
        bootstrap::handle_hotkey_done();
    }
}

extern "C" fn on_window_will_close(_this: &Object, _cmd: Sel, _notification: Id) {
    match OVERLAY_STATE.try_lock() {
        Ok(mut state) => {
            clear_overlay_state(&mut state);
            debug!("Voice chat overlay closed by user");
        }
        Err(_) => {
            debug!("Voice chat overlay close: state lock busy, skipping clear");
        }
    }
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
