//! Action handlers for voice chat overlay
//!
//! Contains Objective-C class registration and action handler functions.

use core_graphics::geometry::{CGPoint, CGRect};
use dispatch::Queue;
use objc::declare::ClassDecl;
use objc::runtime::{Class, Object, Sel};
use objc::{msg_send, sel, sel_impl};
use std::path::PathBuf;
use std::sync::Once;
use tracing::{debug, info};

use crate::config::Config;
use crate::ui::bootstrap;
use crate::ui_helpers::{
    clamp_overlay_position, get_text_field_string, ns_string, set_hidden, set_text_field_string,
};

use super::api::{
    clear_overlay_state, clear_voice_chat_text_impl, commit_last_user_message_impl,
    discard_last_message_impl, filter_drawer, handle_card_copy, handle_card_delete,
    handle_card_edit, handle_card_favorite, reflow_agent_after_resize_impl,
    reflow_overlay_after_resize_impl, send_draft_message_impl, toggle_drawer_favorites_only_impl,
    update_active_tab_impl, update_attach_button_ui,
};
use super::state::{ChatRole, OVERLAY_STATE, Tab};

// Type alias for Objective-C object pointers
pub type Id = *mut Object;

static ACTION_HANDLER_INIT: Once = Once::new();
static mut ACTION_HANDLER_CLASS: *const Class = std::ptr::null();
static WINDOW_DELEGATE_INIT: Once = Once::new();
static mut WINDOW_DELEGATE_CLASS: *const Class = std::ptr::null();
static OVERLAY_WINDOW_INIT: Once = Once::new();
static mut OVERLAY_WINDOW_CLASS: *const Class = std::ptr::null();
static DROP_TARGET_INIT: Once = Once::new();
static mut DROP_TARGET_CLASS: *const Class = std::ptr::null();

const NS_DRAG_OP_COPY: u64 = 1;

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
                sel!(onTabDrawer:),
                on_tab_drawer as extern "C" fn(&Object, Sel, Id),
            );
            decl.add_method(
                sel!(onTabAgent:),
                on_tab_agent as extern "C" fn(&Object, Sel, Id),
            );
            decl.add_method(
                sel!(onTabSettings:),
                on_tab_settings as extern "C" fn(&Object, Sel, Id),
            );
            decl.add_method(sel!(onClose:), on_close as extern "C" fn(&Object, Sel, Id));
            decl.add_method(
                sel!(onCopyLastResponse:),
                on_copy_last_response as extern "C" fn(&Object, Sel, Id),
            );
            decl.add_method(
                sel!(onPasteLastResponse:),
                on_paste_last_response as extern "C" fn(&Object, Sel, Id),
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
                sel!(onToggleFavoritesOnly:),
                on_toggle_favorites_only as extern "C" fn(&Object, Sel, Id),
            );
            decl.add_method(
                sel!(onStartRecording:),
                on_start_recording as extern "C" fn(&Object, Sel, Id),
            );
            decl.add_method(
                sel!(onShowOverlay:),
                on_show_overlay as extern "C" fn(&Object, Sel, Id),
            );
            decl.add_method(
                sel!(onCommitMessage:),
                on_commit_message as extern "C" fn(&Object, Sel, Id),
            );
            decl.add_method(
                sel!(onDiscardMessage:),
                on_discard_message as extern "C" fn(&Object, Sel, Id),
            );
            decl.add_method(
                sel!(onExportMenu:),
                on_export_menu as extern "C" fn(&Object, Sel, Id),
            );
            decl.add_method(
                sel!(onExportAllCopy:),
                on_export_all_copy as extern "C" fn(&Object, Sel, Id),
            );
            decl.add_method(
                sel!(onExportAllSave:),
                on_export_all_save as extern "C" fn(&Object, Sel, Id),
            );
            decl.add_method(
                sel!(onExportAssistantCopy:),
                on_export_assistant_copy as extern "C" fn(&Object, Sel, Id),
            );
            decl.add_method(
                sel!(onExportAssistantSave:),
                on_export_assistant_save as extern "C" fn(&Object, Sel, Id),
            );
            decl.add_method(
                sel!(onMoreMenu:),
                on_more_menu as extern "C" fn(&Object, Sel, Id),
            );
            decl.add_method(
                sel!(onShowShortcuts:),
                on_show_shortcuts as extern "C" fn(&Object, Sel, Id),
            );
            // NSTextView delegate (auto-resize input bar as content grows/shrinks).
            decl.add_method(
                sel!(textDidChange:),
                on_text_did_change as extern "C" fn(&Object, Sel, Id),
            );
            // Intercept Enter → send, Shift+Enter → newline.
            decl.add_method(
                sel!(textView:doCommandBySelector:),
                on_do_command_by_selector as extern "C" fn(&Object, Sel, Id, Sel) -> bool,
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
            decl.add_method(
                sel!(windowDidMove:),
                on_window_did_move as extern "C" fn(&Object, Sel, Id),
            );
            decl.add_method(
                sel!(windowDidEndLiveResize:),
                on_window_did_end_live_resize as extern "C" fn(&Object, Sel, Id),
            );
            decl.add_method(
                sel!(windowDidResize:),
                on_window_did_resize as extern "C" fn(&Object, Sel, Id),
            );
            let cls = decl.register();
            WINDOW_DELEGATE_CLASS = cls;
        });
        WINDOW_DELEGATE_CLASS
    }
}

/// Get or create the overlay window subclass.
///
/// We use a borderless floating window for the overlay. On macOS, borderless NSWindow
/// instances are often not keyable by default, which prevents typing into NSTextField
/// controls. This subclass opts into key/main status so the Agent input field works
/// when the user clicks the overlay.
pub fn overlay_window_class() -> *const Class {
    unsafe {
        OVERLAY_WINDOW_INIT.call_once(|| {
            let superclass = Class::get("NSWindow").expect("NSWindow not found");
            let mut decl = ClassDecl::new("VoiceChatOverlayWindow", superclass)
                .expect("Failed to declare overlay window class");
            decl.add_method(
                sel!(canBecomeKeyWindow),
                can_become_key_window as extern "C" fn(&Object, Sel) -> bool,
            );
            decl.add_method(
                sel!(canBecomeMainWindow),
                can_become_main_window as extern "C" fn(&Object, Sel) -> bool,
            );
            let cls = decl.register();
            OVERLAY_WINDOW_CLASS = cls;
        });
        OVERLAY_WINDOW_CLASS
    }
}

/// Drop target view for attachments (supports dragging files into the Agent input bar).
pub fn drop_target_view_class() -> *const Class {
    unsafe {
        DROP_TARGET_INIT.call_once(|| {
            let superclass = Class::get("NSView").expect("NSView not found");
            let mut decl = ClassDecl::new("VoiceChatAttachmentDropView", superclass)
                .expect("Failed to declare drop target class");
            decl.add_method(
                sel!(draggingEntered:),
                on_dragging_entered as extern "C" fn(&Object, Sel, Id) -> u64,
            );
            decl.add_method(
                sel!(performDragOperation:),
                on_perform_drag_operation as extern "C" fn(&Object, Sel, Id) -> bool,
            );
            let cls = decl.register();
            DROP_TARGET_CLASS = cls;
        });
        DROP_TARGET_CLASS
    }
}

fn extract_paths_from_pasteboard(pasteboard: Id) -> Vec<PathBuf> {
    unsafe {
        let mut out = Vec::new();
        if pasteboard.is_null() {
            return out;
        }

        // Preferred path: read file URLs.
        let ns_url = Class::get("NSURL").unwrap();
        let ns_array = Class::get("NSArray").unwrap();
        let ns_dict = Class::get("NSDictionary").unwrap();
        let ns_number = Class::get("NSNumber").unwrap();

        let classes: Id = msg_send![ns_array, arrayWithObject: ns_url];
        let key = ns_string("NSPasteboardURLReadingFileURLsOnlyKey");
        let yes: Id = msg_send![ns_number, numberWithBool: true];
        let options: Id = msg_send![ns_dict, dictionaryWithObject: yes forKey: key];
        let urls: Id = msg_send![pasteboard, readObjectsForClasses: classes options: options];
        if !urls.is_null() {
            let count: usize = msg_send![urls, count];
            for i in 0..count {
                let url: Id = msg_send![urls, objectAtIndex: i];
                if url.is_null() {
                    continue;
                }
                let ns_path: Id = msg_send![url, path];
                if ns_path.is_null() {
                    continue;
                }
                let c_str: *const i8 = msg_send![ns_path, UTF8String];
                if c_str.is_null() {
                    continue;
                }
                let s = std::ffi::CStr::from_ptr(c_str)
                    .to_string_lossy()
                    .to_string();
                if !s.is_empty() {
                    out.push(PathBuf::from(s));
                }
            }
        }

        // Fallback: legacy filenames pasteboard type.
        if out.is_empty() {
            let filenames_type = ns_string("NSFilenamesPboardType");
            let files: Id = msg_send![pasteboard, propertyListForType: filenames_type];
            if !files.is_null() {
                let count: usize = msg_send![files, count];
                for i in 0..count {
                    let ns_path: Id = msg_send![files, objectAtIndex: i];
                    if ns_path.is_null() {
                        continue;
                    }
                    let c_str: *const i8 = msg_send![ns_path, UTF8String];
                    if c_str.is_null() {
                        continue;
                    }
                    let s = std::ffi::CStr::from_ptr(c_str)
                        .to_string_lossy()
                        .to_string();
                    if !s.is_empty() {
                        out.push(PathBuf::from(s));
                    }
                }
            }
        }

        out
    }
}

extern "C" fn on_dragging_entered(_this: &Object, _cmd: Sel, dragging_info: Id) -> u64 {
    unsafe {
        let pasteboard: Id = msg_send![dragging_info, draggingPasteboard];
        let paths = extract_paths_from_pasteboard(pasteboard);
        if paths.is_empty() { 0 } else { NS_DRAG_OP_COPY }
    }
}

extern "C" fn on_perform_drag_operation(_this: &Object, _cmd: Sel, dragging_info: Id) -> bool {
    unsafe {
        let pasteboard: Id = msg_send![dragging_info, draggingPasteboard];
        let paths = extract_paths_from_pasteboard(pasteboard);
        if paths.is_empty() {
            return false;
        }
        let (btn_ptr, count, names) = {
            let mut state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
            for p in paths {
                if !state.attached_files.contains(&p) {
                    state.attached_files.push(p);
                }
            }
            state.attached_files_last_sent = None;
            let names: Vec<String> = state
                .attached_files
                .iter()
                .filter_map(|p| p.file_name().map(|n| n.to_string_lossy().to_string()))
                .collect();
            (state.agent_attach_button, state.attached_files.len(), names)
        };
        update_attach_button_ui(btn_ptr, count, names);
        true
    }
}

extern "C" fn can_become_key_window(_this: &Object, _cmd: Sel) -> bool {
    true
}

extern "C" fn can_become_main_window(_this: &Object, _cmd: Sel) -> bool {
    true
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

    let (btn_ptr, count, names) = {
        let mut state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
        for p in picked {
            if !state.attached_files.contains(&p) {
                state.attached_files.push(p);
            }
        }
        state.attached_files_last_sent = None;
        let names: Vec<String> = state
            .attached_files
            .iter()
            .filter_map(|p| p.file_name().map(|n| n.to_string_lossy().to_string()))
            .collect();
        (state.agent_attach_button, state.attached_files.len(), names)
    };
    update_attach_button_ui(btn_ptr, count, names);
}

extern "C" fn on_attach_clear(_this: &Object, _cmd: Sel, _sender: Id) {
    let (btn_ptr, count, names) = {
        let mut state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
        state.attached_files.clear();
        state.attached_files_last_sent = None;
        (state.agent_attach_button, 0, Vec::new())
    };
    update_attach_button_ui(btn_ptr, count, names);
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

unsafe fn clamp_overlay_window_to_visible(window: Id) {
    // Don't fight AppKit during live resizing; it can cause flicker/"disappearing" windows.
    let in_live_resize: bool = msg_send![window, inLiveResize];
    if in_live_resize {
        return;
    }

    let ns_screen = Class::get("NSScreen").unwrap();
    let mut screen: Id = msg_send![window, screen];
    if screen.is_null() {
        screen = msg_send![ns_screen, mainScreen];
    }
    if screen.is_null() {
        return;
    }

    let visible_frame: CGRect = msg_send![screen, visibleFrame];
    let frame: CGRect = msg_send![window, frame];
    let margin = 20.0;

    let (x, y) = clamp_overlay_position(
        visible_frame,
        frame.size.width,
        frame.size.height,
        margin,
        frame.origin.x,
        frame.origin.y,
    );

    if (x - frame.origin.x).abs() > 0.5 || (y - frame.origin.y).abs() > 0.5 {
        let _: () = msg_send![window, setFrameOrigin: CGPoint::new(x, y)];
    }
}

extern "C" fn on_window_did_move(_this: &Object, _cmd: Sel, notification: Id) {
    unsafe {
        let window: Id = msg_send![notification, object];
        if window.is_null() {
            return;
        }
        clamp_overlay_window_to_visible(window);
    }
}

extern "C" fn on_window_did_end_live_resize(_this: &Object, _cmd: Sel, notification: Id) {
    unsafe {
        let window: Id = msg_send![notification, object];
        if !window.is_null() {
            clamp_overlay_window_to_visible(window);

            // Cap max size to the current screen's visible frame (handles space/screen changes).
            let ns_screen = Class::get("NSScreen").unwrap();
            let mut screen: Id = msg_send![window, screen];
            if screen.is_null() {
                screen = msg_send![ns_screen, mainScreen];
            }
            if !screen.is_null() {
                let visible: CGRect = msg_send![screen, visibleFrame];
                let _: () = msg_send![window, setContentMaxSize: visible.size];
            }
        }

        // Reflow bubbles to the new width/height.
        Queue::main().exec_async(|| {
            reflow_agent_after_resize_impl();
        });
    }
}

extern "C" fn on_window_did_resize(_this: &Object, _cmd: Sel, _notification: Id) {
    // Keep footer/input aligned during live resizing.
    Queue::main().exec_async(|| {
        reflow_overlay_after_resize_impl();
    });
}

extern "C" fn on_tab_drawer(_this: &Object, _cmd: Sel, _sender: Id) {
    update_active_tab_impl(Tab::Drawer);
    info!("Tab changed to: {:?}", Tab::Drawer);
}

extern "C" fn on_tab_agent(_this: &Object, _cmd: Sel, _sender: Id) {
    update_active_tab_impl(Tab::Agent);
    info!("Tab changed to: {:?}", Tab::Agent);
}

extern "C" fn on_tab_settings(_this: &Object, _cmd: Sel, _sender: Id) {
    crate::show_bootstrap_overlay();
    info!("Settings window opened");
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

extern "C" fn on_paste_last_response(_this: &Object, _cmd: Sel, _sender: Id) {
    let state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
    let text = state
        .messages
        .iter()
        .rev()
        .find(|m| m.role == ChatRole::Assistant)
        .map(|m| m.text.clone());
    let target_app = state.last_target_app.clone();
    drop(state);

    let Some(text) = text else {
        info!("No assistant response to paste");
        return;
    };

    std::thread::spawn(move || {
        #[cfg(target_os = "macos")]
        {
            if let Some(app) = target_app.as_deref() {
                let app = app.replace('"', "\\\"");
                let _ = std::process::Command::new("osascript")
                    .args(["-e", &format!(r#"tell application "{}" to activate"#, app)])
                    .status();
                std::thread::sleep(std::time::Duration::from_millis(80));
            }

            // Best-effort: if activation fails, paste will likely go nowhere useful;
            // clipboard still contains the response.
            if let Err(e) = crate::os::clipboard::paste_text(&text) {
                info!("Paste failed: {}", e);
                copy_to_clipboard(&text);
            }
        }

        #[cfg(not(target_os = "macos"))]
        {
            copy_to_clipboard(&text);
        }
    });
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

extern "C" fn on_toggle_favorites_only(_this: &Object, _cmd: Sel, _sender: Id) {
    toggle_drawer_favorites_only_impl();
    info!("Toggled Drawer favorites-only filter");
}

extern "C" fn on_start_recording(_this: &Object, _cmd: Sel, _sender: Id) {
    crate::controller::request_toggle_recording_start(false);
    info!("CTA: start recording");
}

extern "C" fn on_show_overlay(_this: &Object, _cmd: Sel, _sender: Id) {
    crate::show_voice_chat_overlay();
    info!("CTA: show overlay");
}

extern "C" fn on_commit_message(_this: &Object, _cmd: Sel, _sender: Id) {
    commit_last_user_message_impl();
    info!("Draft message committed");
}

extern "C" fn on_discard_message(_this: &Object, _cmd: Sel, _sender: Id) {
    discard_last_message_impl();
    info!("Draft message discarded");
}

extern "C" fn on_export_menu(this: &Object, _cmd: Sel, sender: Id) {
    unsafe {
        let ns_menu = Class::get("NSMenu").unwrap();
        let ns_menu_item = Class::get("NSMenuItem").unwrap();

        let menu: Id = msg_send![ns_menu, new];
        let target: Id = (this as *const Object) as Id;

        // Submenu: All
        let all_item: Id = msg_send![ns_menu_item, alloc];
        let all_item: Id = msg_send![
            all_item,
            initWithTitle: ns_string("All")
            action: std::ptr::null_mut::<Object>()
            keyEquivalent: ns_string("")
        ];
        let all_menu: Id = msg_send![ns_menu, new];

        let all_copy: Id = msg_send![ns_menu_item, alloc];
        let all_copy: Id = msg_send![
            all_copy,
            initWithTitle: ns_string("Copy as Markdown")
            action: sel!(onExportAllCopy:)
            keyEquivalent: ns_string("")
        ];
        let _: () = msg_send![all_copy, setTarget: target];
        let _: () = msg_send![all_menu, addItem: all_copy];

        let all_save: Id = msg_send![ns_menu_item, alloc];
        let all_save: Id = msg_send![
            all_save,
            initWithTitle: ns_string("Save as Markdown (to history)")
            action: sel!(onExportAllSave:)
            keyEquivalent: ns_string("")
        ];
        let _: () = msg_send![all_save, setTarget: target];
        let _: () = msg_send![all_menu, addItem: all_save];

        let _: () = msg_send![all_item, setSubmenu: all_menu];
        let _: () = msg_send![menu, addItem: all_item];

        // Submenu: Assistant only
        let asst_item: Id = msg_send![ns_menu_item, alloc];
        let asst_item: Id = msg_send![
            asst_item,
            initWithTitle: ns_string("Assistant only")
            action: std::ptr::null_mut::<Object>()
            keyEquivalent: ns_string("")
        ];
        let asst_menu: Id = msg_send![ns_menu, new];

        let asst_copy: Id = msg_send![ns_menu_item, alloc];
        let asst_copy: Id = msg_send![
            asst_copy,
            initWithTitle: ns_string("Copy as Markdown")
            action: sel!(onExportAssistantCopy:)
            keyEquivalent: ns_string("")
        ];
        let _: () = msg_send![asst_copy, setTarget: target];
        let _: () = msg_send![asst_menu, addItem: asst_copy];

        let asst_save: Id = msg_send![ns_menu_item, alloc];
        let asst_save: Id = msg_send![
            asst_save,
            initWithTitle: ns_string("Save as Markdown (to history)")
            action: sel!(onExportAssistantSave:)
            keyEquivalent: ns_string("")
        ];
        let _: () = msg_send![asst_save, setTarget: target];
        let _: () = msg_send![asst_menu, addItem: asst_save];

        let _: () = msg_send![asst_item, setSubmenu: asst_menu];
        let _: () = msg_send![menu, addItem: asst_item];

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

extern "C" fn on_export_all_copy(_this: &Object, _cmd: Sel, _sender: Id) {
    let md = super::api::export_chat_markdown(false);
    copy_to_clipboard(&md);
    info!("Exported chat (all) to clipboard as Markdown");
}

extern "C" fn on_export_all_save(_this: &Object, _cmd: Sel, _sender: Id) {
    if let Some(path) = super::api::save_chat_markdown_to_history(false) {
        info!("Saved chat (all) export to {}", path.display());
        super::api::refresh_drawer();
    } else {
        info!("Failed to save chat (all) export");
    }
}

extern "C" fn on_export_assistant_copy(_this: &Object, _cmd: Sel, _sender: Id) {
    let md = super::api::export_chat_markdown(true);
    copy_to_clipboard(&md);
    info!("Exported chat (assistant-only) to clipboard as Markdown");
}

extern "C" fn on_export_assistant_save(_this: &Object, _cmd: Sel, _sender: Id) {
    if let Some(path) = super::api::save_chat_markdown_to_history(true) {
        info!("Saved chat (assistant-only) export to {}", path.display());
        super::api::refresh_drawer();
    } else {
        info!("Failed to save chat (assistant-only) export");
    }
}

extern "C" fn on_show_shortcuts(_this: &Object, _cmd: Sel, _sender: Id) {
    let config = Config::load();
    let (hold, toggle) = super::shortcuts_lines(config.hold_mods, config.toggle_trigger);
    unsafe {
        let ns_alert = Class::get("NSAlert").unwrap();
        let alert: Id = msg_send![ns_alert, new];
        let _: () = msg_send![alert, setMessageText: ns_string("Keyboard Shortcuts")];
        let _: () =
            msg_send![alert, setInformativeText: ns_string(&format!("{}\n{}", hold, toggle))];
        let _: () = msg_send![alert, setAlertStyle: 1_isize]; // NSAlertStyleInformational
        let _: () = msg_send![alert, runModal];
    }
}

extern "C" fn on_more_menu(this: &Object, _cmd: Sel, sender: Id) {
    unsafe {
        let ns_menu = Class::get("NSMenu").unwrap();
        let ns_menu_item = Class::get("NSMenuItem").unwrap();

        let menu: Id = msg_send![ns_menu, new];
        let target: Id = (this as *const Object) as Id;

        let new_thread: Id = msg_send![ns_menu_item, alloc];
        let new_thread: Id = msg_send![
            new_thread,
            initWithTitle: ns_string("New thread")
            action: sel!(onNewThread:)
            keyEquivalent: ns_string("")
        ];
        let _: () = msg_send![new_thread, setTarget: target];
        let _: () = msg_send![menu, addItem: new_thread];

        let sep: Id = msg_send![ns_menu_item, separatorItem];
        let _: () = msg_send![menu, addItem: sep];

        let copy_last: Id = msg_send![ns_menu_item, alloc];
        let copy_last: Id = msg_send![
            copy_last,
            initWithTitle: ns_string("Copy last response")
            action: sel!(onCopyLastResponse:)
            keyEquivalent: ns_string("")
        ];
        let _: () = msg_send![copy_last, setTarget: target];
        let _: () = msg_send![menu, addItem: copy_last];

        let paste_last: Id = msg_send![ns_menu_item, alloc];
        let paste_last: Id = msg_send![
            paste_last,
            initWithTitle: ns_string("Paste last response")
            action: sel!(onPasteLastResponse:)
            keyEquivalent: ns_string("")
        ];
        let _: () = msg_send![paste_last, setTarget: target];
        let _: () = msg_send![menu, addItem: paste_last];

        let sep2: Id = msg_send![ns_menu_item, separatorItem];
        let _: () = msg_send![menu, addItem: sep2];

        let shortcuts_item: Id = msg_send![ns_menu_item, alloc];
        let shortcuts_item: Id = msg_send![
            shortcuts_item,
            initWithTitle: ns_string("Keyboard Shortcuts")
            action: sel!(onShowShortcuts:)
            keyEquivalent: ns_string("?")
        ];
        let _: () = msg_send![shortcuts_item, setTarget: target];
        let _: () = msg_send![shortcuts_item, setKeyEquivalentModifierMask: (1u64 << 20)];
        let _: () = msg_send![menu, addItem: shortcuts_item];

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

extern "C" fn on_text_did_change(_this: &Object, _cmd: Sel, _notification: Id) {
    // Runs on main thread. Keep lightweight and only re-layout when height changes.
    super::api::resize_agent_input_to_draft();
}

/// NSTextView delegate: intercept Enter to send, allow Shift+Enter for newline.
/// Respects `agent_enter_sends` config:
///   true  → Enter sends, Shift+Enter newline (default / Discord-style)
///   false → Enter newline, Cmd+Enter sends   (Mail / Messages-style)
extern "C" fn on_do_command_by_selector(
    _this: &Object,
    _cmd: Sel,
    _text_view: Id,
    selector: Sel,
) -> bool {
    if selector == sel!(insertNewline:) {
        let (shift_held, cmd_held) = unsafe {
            let ns_app = Class::get("NSApplication").unwrap();
            let app: Id = msg_send![ns_app, sharedApplication];
            let event: Id = msg_send![app, currentEvent];
            if event.is_null() {
                (false, false)
            } else {
                let flags: u64 = msg_send![event, modifierFlags];
                // NSEventModifierFlagShift = 1 << 17
                // NSEventModifierFlagCommand = 1 << 20
                ((flags & (1 << 17)) != 0, (flags & (1 << 20)) != 0)
            }
        };
        let config = Config::load();
        let should_send = if config.agent_enter_sends {
            !shift_held // Enter sends, Shift+Enter newline
        } else {
            cmd_held // Cmd+Enter sends, Enter newline
        };
        if should_send {
            send_draft_message_impl();
            return true; // Handled: send message.
        }
        return false; // Let NSTextView insert a newline.
    }
    false // All other commands: default behaviour.
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
