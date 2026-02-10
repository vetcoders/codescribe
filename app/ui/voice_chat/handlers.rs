//! Action handlers for voice chat overlay
//!
//! Contains Objective-C class registration and action handler functions.

use core_graphics::geometry::{CGPoint, CGRect, CGSize};
use dispatch::Queue;
use objc::declare::ClassDecl;
use objc::runtime::{Class, Object, Sel};
use objc::{msg_send, sel, sel_impl};
use std::path::PathBuf;
use std::sync::Once;
use tracing::{debug, info};

use codescribe_core::attachment::{Attachment, AttachmentSource, AttachmentStore};
use codescribe_core::config::UserSettings;

use crate::config::Config;
use crate::ui::bootstrap;
use crate::ui_helpers::{
    clamp_overlay_position, get_text_field_string, ns_string, set_hidden, set_text_field_string,
};

use super::api::{
    clear_overlay_state, clear_voice_chat_text_impl, commit_last_user_message_impl,
    discard_last_message_impl, filter_drawer, handle_card_copy, handle_card_delete,
    handle_card_edit, handle_card_favorite, reflow_agent_after_resize_impl,
    reflow_overlay_after_resize_impl, render_attachment_chips, send_draft_message_impl,
    toggle_drawer_favorites_only_impl, update_active_tab_impl, update_attach_button_ui,
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
            // Connector actions (GitHub, URL)
            decl.add_method(
                sel!(onAttachGitHub:),
                on_attach_github as extern "C" fn(&Object, Sel, Id),
            );
            decl.add_method(
                sel!(onAttachURL:),
                on_attach_url as extern "C" fn(&Object, Sel, Id),
            );
            // Attachment chip actions
            decl.add_method(
                sel!(onChipClick:),
                on_chip_click as extern "C" fn(&Object, Sel, Id),
            );
            decl.add_method(
                sel!(onChipRemove:),
                on_chip_remove as extern "C" fn(&Object, Sel, Id),
            );
            decl.add_method(
                sel!(onChipPreview:),
                on_chip_preview as extern "C" fn(&Object, Sel, Id),
            );
            decl.add_method(
                sel!(onChipReveal:),
                on_chip_reveal as extern "C" fn(&Object, Sel, Id),
            );
            decl.add_method(
                sel!(onChipCopyPath:),
                on_chip_copy_path as extern "C" fn(&Object, Sel, Id),
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
            decl.add_method(
                sel!(performKeyEquivalent:),
                perform_key_equivalent as extern "C" fn(&Object, Sel, Id) -> bool,
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
                if !state.attachments.iter().any(|a| a.same_path(&p)) {
                    state
                        .attachments
                        .push(Attachment::from_path(p, AttachmentSource::DragDrop));
                }
            }
            state.attachments_last_sent = None;
            render_attachment_chips(&mut state);
            let names: Vec<String> = state
                .attachments
                .iter()
                .map(|a| a.display_name.clone())
                .collect();
            (state.agent_attach_button, state.attachments.len(), names)
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

extern "C" fn perform_key_equivalent(_this: &Object, _cmd: Sel, event: Id) -> bool {
    unsafe {
        let flags: u64 = msg_send![event, modifierFlags];
        let has_cmd = (flags & (1 << 20)) != 0; // NSEventModifierFlagCommand
        if !has_cmd {
            return false;
        }

        let chars: Id = msg_send![event, charactersIgnoringModifiers];
        if chars.is_null() {
            return false;
        }
        let c_str: *const i8 = msg_send![chars, UTF8String];
        if c_str.is_null() {
            return false;
        }
        let key = std::ffi::CStr::from_ptr(c_str).to_string_lossy();

        match key.as_ref() {
            "=" | "+" => {
                adjust_chat_zoom(0.125);
                true
            }
            "-" => {
                adjust_chat_zoom(-0.125);
                true
            }
            "0" => {
                set_chat_zoom(1.0);
                true
            }
            _ => false,
        }
    }
}

/// Monotonic generation counter for zoom save debounce.
static ZOOM_SAVE_GEN: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

fn adjust_chat_zoom(delta: f64) {
    let zoom = {
        let mut state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
        state.zoom_level = (state.zoom_level + delta).clamp(0.75, 2.0);
        state.zoom_level
    };
    reflow_agent_after_resize_impl();
    schedule_zoom_save(zoom);
}

fn set_chat_zoom(level: f64) {
    {
        let mut state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
        state.zoom_level = level;
    }
    reflow_agent_after_resize_impl();
    schedule_zoom_save(level);
}

/// Debounced save: waits 500ms, then saves only if no newer zoom change occurred.
fn schedule_zoom_save(zoom: f64) {
    let generation = ZOOM_SAVE_GEN.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
    std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_millis(500));
        if ZOOM_SAVE_GEN.load(std::sync::atomic::Ordering::Relaxed) != generation {
            return; // newer zoom change supersedes
        }
        let mut settings = UserSettings::load();
        settings.chat_zoom = if (zoom - 1.0).abs() < 0.01 {
            None
        } else {
            Some(zoom)
        };
        let _ = settings.save();
    });
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

        let github: Id = msg_send![ns_menu_item, alloc];
        let github: Id = msg_send![
            github,
            initWithTitle: ns_string("Attach from GitHub…")
            action: sel!(onAttachGitHub:)
            keyEquivalent: ns_string("")
        ];
        let _: () = msg_send![github, setTarget: target];
        let _: () = msg_send![menu, addItem: github];

        let url: Id = msg_send![ns_menu_item, alloc];
        let url: Id = msg_send![
            url,
            initWithTitle: ns_string("Attach from URL…")
            action: sel!(onAttachURL:)
            keyEquivalent: ns_string("")
        ];
        let _: () = msg_send![url, setTarget: target];
        let _: () = msg_send![menu, addItem: url];

        let count = {
            let state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
            state.attachments.len()
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
            if !state.attachments.iter().any(|a| a.same_path(&p)) {
                state
                    .attachments
                    .push(Attachment::from_path(p, AttachmentSource::FilePicker));
            }
        }
        state.attachments_last_sent = None;
        render_attachment_chips(&mut state);
        let names: Vec<String> = state
            .attachments
            .iter()
            .map(|a| a.display_name.clone())
            .collect();
        (state.agent_attach_button, state.attachments.len(), names)
    };
    update_attach_button_ui(btn_ptr, count, names);
}

extern "C" fn on_attach_clear(_this: &Object, _cmd: Sel, _sender: Id) {
    let (btn_ptr, count, names) = {
        let mut state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
        state.attachments.clear();
        state.attachments_last_sent = None;
        render_attachment_chips(&mut state);
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

            // Cap max size: width 1000px, height = screen visible height.
            let ns_screen = Class::get("NSScreen").unwrap();
            let mut screen: Id = msg_send![window, screen];
            if screen.is_null() {
                screen = msg_send![ns_screen, mainScreen];
            }
            if !screen.is_null() {
                let visible: CGRect = msg_send![screen, visibleFrame];
                let max_w = visible.size.width.min(1000.0);
                let _: () =
                    msg_send![window, setContentMaxSize: CGSize::new(max_w, visible.size.height)];
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

// ═══════════════════════════════════════════════════════════
// Connector Handlers (GitHub, URL)
// ═══════════════════════════════════════════════════════════

/// Show an input dialog and fetch a file from GitHub, adding it as an attachment.
extern "C" fn on_attach_github(_this: &Object, _cmd: Sel, _sender: Id) {
    let input = show_text_input_dialog(
        "Attach from GitHub",
        "Enter a GitHub file URL or spec:\n\
         \u{2022} https://github.com/owner/repo/blob/branch/path\n\
         \u{2022} owner/repo@branch:path/to/file",
        "https://github.com/...",
    );
    let Some(input) = input else { return };
    let input = input.trim().to_string();
    if input.is_empty() {
        return;
    }

    let Some(gh_ref) = codescribe_core::connectors::github::parse_github_ref(&input) else {
        show_error_alert(
            "Invalid GitHub reference",
            &format!("Could not parse: {input}"),
        );
        return;
    };

    // Fetch in background thread, then add attachment on main thread.
    std::thread::spawn(move || {
        let rt = match tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
        {
            Ok(rt) => rt,
            Err(e) => {
                let msg = format!("Failed to start async runtime: {e}");
                Queue::main().exec_async(move || show_error_alert("GitHub Fetch Error", &msg));
                return;
            }
        };
        let result = rt.block_on(async {
            let token = codescribe_core::connectors::github::load_github_token();
            codescribe_core::connectors::github::fetch_github_blob(&gh_ref, token.as_deref()).await
        });
        match result {
            Ok((data, filename)) => {
                match codescribe_core::attachment::AttachmentStore::save_fetched(
                    &data, &filename, "gh",
                ) {
                    Ok(path) => {
                        Queue::main().exec_async(move || {
                            let (btn_ptr, count, names) = {
                                let mut state =
                                    OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
                                state.attachments.push(Attachment::from_path(
                                    path,
                                    AttachmentSource::Connector("github".into()),
                                ));
                                state.attachments_last_sent = None;
                                render_attachment_chips(&mut state);
                                let names: Vec<String> = state
                                    .attachments
                                    .iter()
                                    .map(|a| a.display_name.clone())
                                    .collect();
                                (state.agent_attach_button, state.attachments.len(), names)
                            };
                            update_attach_button_ui(btn_ptr, count, names);
                        });
                    }
                    Err(e) => {
                        let msg = format!("Failed to save: {e}");
                        Queue::main()
                            .exec_async(move || show_error_alert("GitHub Fetch Error", &msg));
                    }
                }
            }
            Err(e) => {
                let msg = format!("{e}");
                Queue::main().exec_async(move || show_error_alert("GitHub Fetch Error", &msg));
            }
        }
    });
}

/// Show an input dialog and fetch content from a URL, adding it as an attachment.
extern "C" fn on_attach_url(_this: &Object, _cmd: Sel, _sender: Id) {
    let input = show_text_input_dialog(
        "Attach from URL",
        "Enter a URL to fetch as attachment context:",
        "https://...",
    );
    let Some(input) = input else { return };
    let input = input.trim().to_string();
    if input.is_empty() {
        return;
    }
    if !codescribe_core::connectors::web::looks_like_url(&input) {
        show_error_alert("Invalid URL", "URL must start with http:// or https://");
        return;
    }

    // Fetch in background thread, then add attachment on main thread.
    std::thread::spawn(move || {
        let rt = match tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
        {
            Ok(rt) => rt,
            Err(e) => {
                let msg = format!("Failed to start async runtime: {e}");
                Queue::main().exec_async(move || show_error_alert("URL Fetch Error", &msg));
                return;
            }
        };
        let result = rt.block_on(codescribe_core::connectors::web::fetch_url_as_text(&input));
        match result {
            Ok((text, title)) => {
                let display_name = if title.is_empty() {
                    "webpage.txt".to_string()
                } else {
                    format!("{}.txt", title.chars().take(40).collect::<String>())
                };
                match codescribe_core::attachment::AttachmentStore::save_fetched(
                    text.as_bytes(),
                    &display_name,
                    "url",
                ) {
                    Ok(path) => {
                        Queue::main().exec_async(move || {
                            let (btn_ptr, count, names) = {
                                let mut state =
                                    OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
                                let att = Attachment::with_kind(
                                    path,
                                    codescribe_core::attachment::AttachmentKind::UrlSnapshot,
                                    AttachmentSource::Connector("web".into()),
                                );
                                state.attachments.push(att);
                                state.attachments_last_sent = None;
                                render_attachment_chips(&mut state);
                                let names: Vec<String> = state
                                    .attachments
                                    .iter()
                                    .map(|a| a.display_name.clone())
                                    .collect();
                                (state.agent_attach_button, state.attachments.len(), names)
                            };
                            update_attach_button_ui(btn_ptr, count, names);
                        });
                    }
                    Err(e) => {
                        let msg = format!("Failed to save: {e}");
                        Queue::main().exec_async(move || show_error_alert("URL Fetch Error", &msg));
                    }
                }
            }
            Err(e) => {
                let msg = format!("{e}");
                Queue::main().exec_async(move || show_error_alert("URL Fetch Error", &msg));
            }
        }
    });
}

/// Show a modal text input dialog using NSAlert with an accessory NSTextField.
/// Returns the entered text, or None if the user cancelled.
fn show_text_input_dialog(title: &str, message: &str, placeholder: &str) -> Option<String> {
    unsafe {
        let ns_alert = Class::get("NSAlert").unwrap();
        let alert: Id = msg_send![ns_alert, new];
        let _: () = msg_send![alert, setMessageText: ns_string(title)];
        let _: () = msg_send![alert, setInformativeText: ns_string(message)];
        let _: () = msg_send![alert, addButtonWithTitle: ns_string("OK")];
        let _: () = msg_send![alert, addButtonWithTitle: ns_string("Cancel")];
        let _: () = msg_send![alert, setAlertStyle: 1_isize]; // NSAlertStyleInformational

        // Add a text field as accessory view.
        let ns_text_field = Class::get("NSTextField").unwrap();
        let field: Id = msg_send![ns_text_field, alloc];
        let field_frame = CGRect::new(
            &CGPoint::new(0.0, 0.0),
            &core_graphics::geometry::CGSize::new(300.0, 24.0),
        );
        let field: Id = msg_send![field, initWithFrame: field_frame];
        let _: () = msg_send![field, setPlaceholderString: ns_string(placeholder)];
        let _: () = msg_send![alert, setAccessoryView: field];

        // Make the text field first responder so it's focused.
        let window: Id = msg_send![alert, window];
        let _: () = msg_send![window, setInitialFirstResponder: field];

        // NSModalResponseOK (first button) = 1000
        let response: isize = msg_send![alert, runModal];
        if response != 1000 {
            return None;
        }
        let text: Id = msg_send![field, stringValue];
        if text.is_null() {
            return None;
        }
        let c_str: *const i8 = msg_send![text, UTF8String];
        if c_str.is_null() {
            return None;
        }
        let s = std::ffi::CStr::from_ptr(c_str)
            .to_string_lossy()
            .to_string();
        Some(s)
    }
}

/// Show a simple error alert.
fn show_error_alert(title: &str, message: &str) {
    unsafe {
        let ns_alert = Class::get("NSAlert").unwrap();
        let alert: Id = msg_send![ns_alert, new];
        let _: () = msg_send![alert, setMessageText: ns_string(title)];
        let _: () = msg_send![alert, setInformativeText: ns_string(message)];
        let _: () = msg_send![alert, setAlertStyle: 2_isize]; // NSAlertStyleCritical
        let _: () = msg_send![alert, runModal];
    }
}

// ═══════════════════════════════════════════════════════════
// Attachment Chip Handlers
// ═══════════════════════════════════════════════════════════

/// Chip body click → show context menu with Preview / Remove / Reveal / Copy Path.
extern "C" fn on_chip_click(_this: &Object, _cmd: Sel, sender: Id) {
    let index: isize = unsafe { msg_send![sender, tag] };
    let index = index.max(0) as usize;

    unsafe {
        let ns_menu = Class::get("NSMenu").unwrap();
        let ns_menu_item = Class::get("NSMenuItem").unwrap();
        let menu: Id = msg_send![ns_menu, new];
        let handler = {
            let state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
            state.action_handler
        };
        let Some(handler_ptr) = handler else { return };
        let target = handler_ptr as Id;

        let preview: Id = msg_send![ns_menu_item, alloc];
        let preview: Id = msg_send![
            preview,
            initWithTitle: ns_string("Preview (QuickLook)")
            action: sel!(onChipPreview:)
            keyEquivalent: ns_string("")
        ];
        let _: () = msg_send![preview, setTarget: target];
        let _: () = msg_send![preview, setTag: index as isize];
        let _: () = msg_send![menu, addItem: preview];

        let remove: Id = msg_send![ns_menu_item, alloc];
        let remove: Id = msg_send![
            remove,
            initWithTitle: ns_string("Remove")
            action: sel!(onChipRemove:)
            keyEquivalent: ns_string("")
        ];
        let _: () = msg_send![remove, setTarget: target];
        let _: () = msg_send![remove, setTag: index as isize];
        let _: () = msg_send![menu, addItem: remove];

        let sep: Id = msg_send![ns_menu_item, separatorItem];
        let _: () = msg_send![menu, addItem: sep];

        let reveal: Id = msg_send![ns_menu_item, alloc];
        let reveal: Id = msg_send![
            reveal,
            initWithTitle: ns_string("Reveal in Finder")
            action: sel!(onChipReveal:)
            keyEquivalent: ns_string("")
        ];
        let _: () = msg_send![reveal, setTarget: target];
        let _: () = msg_send![reveal, setTag: index as isize];
        let _: () = msg_send![menu, addItem: reveal];

        let copy_path: Id = msg_send![ns_menu_item, alloc];
        let copy_path: Id = msg_send![
            copy_path,
            initWithTitle: ns_string("Copy Path")
            action: sel!(onChipCopyPath:)
            keyEquivalent: ns_string("")
        ];
        let _: () = msg_send![copy_path, setTarget: target];
        let _: () = msg_send![copy_path, setTag: index as isize];
        let _: () = msg_send![menu, addItem: copy_path];

        // Pop up at mouse location.
        let ns_event = Class::get("NSEvent").unwrap();
        let location: CGPoint = msg_send![ns_event, mouseLocation];
        let _: bool = msg_send![
            menu,
            popUpMenuPositioningItem: std::ptr::null_mut::<Object>()
            atLocation: location
            inView: std::ptr::null_mut::<Object>()
        ];
    }
}

extern "C" fn on_chip_remove(_this: &Object, _cmd: Sel, sender: Id) {
    let index: isize = unsafe { msg_send![sender, tag] };
    let index = index.max(0) as usize;
    super::api::remove_attachment_at(index);
}

extern "C" fn on_chip_preview(_this: &Object, _cmd: Sel, sender: Id) {
    let index: isize = unsafe { msg_send![sender, tag] };
    let index = index.max(0) as usize;
    let path = {
        let state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
        state.attachments.get(index).map(|a| a.path.clone())
    };
    if let Some(path) = path {
        // Use macOS QuickLook for native preview.
        std::thread::spawn(move || {
            let _ = std::process::Command::new("qlmanage")
                .arg("-p")
                .arg(&path)
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .spawn();
        });
    }
}

extern "C" fn on_chip_reveal(_this: &Object, _cmd: Sel, sender: Id) {
    let index: isize = unsafe { msg_send![sender, tag] };
    let index = index.max(0) as usize;
    let path = {
        let state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
        state.attachments.get(index).map(|a| a.path.clone())
    };
    if let Some(path) = path {
        let _ = std::process::Command::new("open")
            .arg("-R")
            .arg(&path)
            .spawn();
    }
}

extern "C" fn on_chip_copy_path(_this: &Object, _cmd: Sel, sender: Id) {
    let index: isize = unsafe { msg_send![sender, tag] };
    let index = index.max(0) as usize;
    let path = {
        let state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
        state
            .attachments
            .get(index)
            .map(|a| a.path.display().to_string())
    };
    if let Some(path) = path {
        copy_to_clipboard(&path);
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
    // ── Cmd+V paste interception ──
    // Intercept paste: to handle file URLs and standalone images as attachments.
    // Text paste (with or without accompanying image) falls through to default.
    if selector == sel!(paste:) {
        let handled = unsafe { try_paste_as_attachment() };
        if handled {
            return true;
        }
        return false; // default NSTextView paste
    }

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
// Paste-as-attachment
// ═══════════════════════════════════════════════════════════

/// Check the general pasteboard and, if it contains file URLs or a standalone image
/// (no accompanying text), treat the paste as an attachment instead of text insertion.
///
/// Returns `true` when the paste was consumed as an attachment (caller should suppress
/// the default NSTextView paste), or `false` to let the default paste proceed.
///
/// # Safety
/// Must be called on the main thread. Uses Objective-C messaging.
unsafe fn try_paste_as_attachment() -> bool {
    let ns_pasteboard = Class::get("NSPasteboard").unwrap();
    let pasteboard: Id = msg_send![ns_pasteboard, generalPasteboard];

    // 1. File URLs → always treat as attachments
    let file_paths = extract_paths_from_pasteboard(pasteboard);
    if !file_paths.is_empty() {
        let (btn_ptr, count, names) = {
            let mut state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
            for p in file_paths {
                if !state.attachments.iter().any(|a| a.same_path(&p)) {
                    state
                        .attachments
                        .push(Attachment::from_path(p, AttachmentSource::Clipboard));
                }
            }
            state.attachments_last_sent = None;
            render_attachment_chips(&mut state);
            let names: Vec<String> = state
                .attachments
                .iter()
                .map(|a| a.display_name.clone())
                .collect();
            (state.agent_attach_button, state.attachments.len(), names)
        };
        update_attach_button_ui(btn_ptr, count, names);
        debug!("Paste intercepted: {} file(s) attached", count);
        return true;
    }

    // 2. Check for image data WITHOUT accompanying text
    let has_image = unsafe { pasteboard_has_type(pasteboard, "public.tiff") }
        || unsafe { pasteboard_has_type(pasteboard, "public.png") };
    let has_text = unsafe { pasteboard_has_type(pasteboard, "public.utf8-plain-text") };

    if has_image && !has_text {
        // Read PNG data from pasteboard (try PNG first, then TIFF→PNG conversion)
        if let Some(image_data) = unsafe { read_image_from_pasteboard(pasteboard) } {
            match AttachmentStore::save_clipboard_image(&image_data, "png") {
                Ok(path) => {
                    let (btn_ptr, count, names) = {
                        let mut state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
                        state
                            .attachments
                            .push(Attachment::from_path(path, AttachmentSource::Clipboard));
                        state.attachments_last_sent = None;
                        render_attachment_chips(&mut state);
                        let names: Vec<String> = state
                            .attachments
                            .iter()
                            .map(|a| a.display_name.clone())
                            .collect();
                        (state.agent_attach_button, state.attachments.len(), names)
                    };
                    update_attach_button_ui(btn_ptr, count, names);
                    debug!("Paste intercepted: clipboard image saved as attachment");
                    return true;
                }
                Err(e) => {
                    info!("Failed to save clipboard image: {e}");
                    // Fall through to default paste
                }
            }
        }
    }

    // 3. Text paste (with or without image) → default behaviour
    false
}

/// Check if the pasteboard contains a given UTI type.
///
/// # Safety
/// Requires main thread and valid pasteboard pointer.
unsafe fn pasteboard_has_type(pasteboard: Id, uti: &str) -> bool {
    let ns_array = Class::get("NSArray").unwrap();
    let type_str = ns_string(uti);
    let types: Id = msg_send![ns_array, arrayWithObject: type_str];
    let available: Id = msg_send![pasteboard, availableTypeFromArray: types];
    !available.is_null()
}

/// Read image data from the pasteboard as PNG bytes.
///
/// Tries `public.png` first. Falls back to `public.tiff` and converts to PNG
/// via `NSBitmapImageRep`.
///
/// # Safety
/// Requires main thread and valid pasteboard pointer.
unsafe fn read_image_from_pasteboard(pasteboard: Id) -> Option<Vec<u8>> {
    // Try PNG first
    let png_type = ns_string("public.png");
    let png_data: Id = msg_send![pasteboard, dataForType: png_type];
    if !png_data.is_null() {
        let length: usize = msg_send![png_data, length];
        let bytes: *const u8 = msg_send![png_data, bytes];
        if !bytes.is_null() && length > 0 {
            return Some(unsafe { std::slice::from_raw_parts(bytes, length) }.to_vec());
        }
    }

    // Fallback: TIFF → convert to PNG via NSBitmapImageRep
    let tiff_type = ns_string("public.tiff");
    let tiff_data: Id = msg_send![pasteboard, dataForType: tiff_type];
    if tiff_data.is_null() {
        return None;
    }

    let ns_bitmap = Class::get("NSBitmapImageRep")?;
    let rep: Id = msg_send![ns_bitmap, imageRepWithData: tiff_data];
    if rep.is_null() {
        return None;
    }

    // NSBitmapImageFileType.png = 4
    let png_file_type: usize = 4;
    let nil: Id = std::ptr::null_mut();
    let result: Id = msg_send![
        rep,
        representationUsingType: png_file_type
        properties: nil
    ];
    if result.is_null() {
        return None;
    }

    let length: usize = msg_send![result, length];
    let bytes: *const u8 = msg_send![result, bytes];
    if bytes.is_null() || length == 0 {
        return None;
    }
    Some(unsafe { std::slice::from_raw_parts(bytes, length) }.to_vec())
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
