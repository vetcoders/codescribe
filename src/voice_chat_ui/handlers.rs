//! Action handlers for voice chat overlay
//!
//! Contains Objective-C class registration and action handler functions.

use objc::declare::ClassDecl;
use objc::runtime::{Class, Object, Sel};
use objc::{msg_send, sel, sel_impl};
use std::sync::Once;
use tracing::{debug, info};

use crate::ui_helpers::{animate_window_width, ns_string, open_file_in_editor, set_hidden};

use super::api::{clear_overlay_state, send_draft_message_impl};
use super::state::{ChatRole, OVERLAY_STATE};

// Type alias for Objective-C object pointers
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
                sel!(onToggleAutoSend:),
                on_toggle_auto_send as extern "C" fn(&Object, Sel, Id),
            );
            decl.add_method(
                sel!(onTabChanged:),
                on_tab_changed as extern "C" fn(&Object, Sel, Id),
            );
            decl.add_method(
                sel!(onCopyLastResponse:),
                on_copy_last_response as extern "C" fn(&Object, Sel, Id),
            );
            decl.add_method(
                sel!(onAttach:),
                on_attach as extern "C" fn(&Object, Sel, Id),
            );
            decl.add_method(
                sel!(onToggleCollapse:),
                on_toggle_collapse as extern "C" fn(&Object, Sel, Id),
            );
            decl.add_method(
                sel!(onDraftEdit:),
                on_draft_edit as extern "C" fn(&Object, Sel, Id),
            );
            decl.add_method(
                sel!(onDraftCopy:),
                on_draft_copy as extern "C" fn(&Object, Sel, Id),
            );
            decl.add_method(
                sel!(onCopyMessage:),
                on_copy_message as extern "C" fn(&Object, Sel, Id),
            );
            // Settings tab handlers
            decl.add_method(
                sel!(onSettingsAiFormatting:),
                on_settings_ai_formatting as extern "C" fn(&Object, Sel, Id),
            );
            decl.add_method(
                sel!(onSettingsEditConfig:),
                on_settings_edit_config as extern "C" fn(&Object, Sel, Id),
            );
            decl.add_method(
                sel!(onSettingsEditPrompt:),
                on_settings_edit_prompt as extern "C" fn(&Object, Sel, Id),
            );
            decl.add_method(
                sel!(onSettingsOpenPromptsFolder:),
                on_settings_open_prompts_folder as extern "C" fn(&Object, Sel, Id),
            );
            decl.add_method(
                sel!(onSettingsResetContext:),
                on_settings_reset_context as extern "C" fn(&Object, Sel, Id),
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

extern "C" fn on_window_will_close(_this: &Object, _cmd: Sel, _notification: Id) {
    // Window is closing (user clicked close). Clear state to avoid use-after-free.
    let mut state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
    clear_overlay_state(&mut state);
    debug!("Voice chat overlay closed by user");
}

extern "C" fn on_toggle_auto_send(_this: &Object, _cmd: Sel, sender: Id) {
    unsafe {
        let state_val: isize = msg_send![sender, state];
        let is_on = state_val == 1; // NSControlStateValueOn = 1
        let mut state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
        state.auto_send_enabled = is_on;
        info!("Auto-send toggled: {}", is_on);
    }
}

extern "C" fn on_attach(_this: &Object, _cmd: Sel, _sender: Id) {
    unsafe {
        let ns_open_panel = Class::get("NSOpenPanel").unwrap();
        let panel: Id = msg_send![ns_open_panel, openPanel];

        // Configure panel
        let _: () = msg_send![panel, setCanChooseFiles: true];
        let _: () = msg_send![panel, setCanChooseDirectories: false];
        let _: () = msg_send![panel, setAllowsMultipleSelection: true];

        let ns_string_class = Class::get("NSString").unwrap();
        let title: Id =
            msg_send![ns_string_class, stringWithUTF8String: c"Select files to attach".as_ptr()];
        let _: () = msg_send![panel, setTitle: title];

        // Run modal
        let result: isize = msg_send![panel, runModal];

        // NSModalResponseOK = 1
        if result == 1 {
            let urls: Id = msg_send![panel, URLs];
            let count: usize = msg_send![urls, count];

            let mut state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
            for i in 0..count {
                let url: Id = msg_send![urls, objectAtIndex: i];
                let path: Id = msg_send![url, path];
                let path_cstr: *const i8 = msg_send![path, UTF8String];
                if !path_cstr.is_null() {
                    let path_str = std::ffi::CStr::from_ptr(path_cstr).to_string_lossy();
                    state
                        .attachments
                        .push(std::path::PathBuf::from(path_str.to_string()));
                    info!("Attached: {}", path_str);
                }
            }

            // Update button to show count
            if let Some(btn_ptr) = state.attach_button {
                let btn = btn_ptr as Id;
                let title_str = format!("📎{}", state.attachments.len());
                let mut c_str = title_str.as_bytes().to_vec();
                c_str.push(0);
                let title: Id = msg_send![ns_string_class, stringWithUTF8String: c_str.as_ptr()];
                let _: () = msg_send![btn, setTitle: title];
            }
        }
    }
}

extern "C" fn on_tab_changed(_this: &Object, _cmd: Sel, sender: Id) {
    unsafe {
        let selected: isize = msg_send![sender, selectedSegment];
        let mut state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
        state.selected_tab = selected as usize;

        // Switch visible content between Drafts (0) and Settings (1)
        let show_drafts = selected == 0;

        if let Some(drafts_ptr) = state.drafts_scroll_view {
            set_hidden(drafts_ptr as Id, !show_drafts);
        }
        if let Some(settings_ptr) = state.settings_scroll_view {
            set_hidden(settings_ptr as Id, show_drafts);
        }

        info!(
            "Tab changed to: {}",
            if show_drafts { "Drafts" } else { "Settings" }
        );
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

extern "C" fn on_toggle_collapse(_this: &Object, _cmd: Sel, sender: Id) {
    // Window dimensions for animation
    const EXPANDED_WIDTH: f64 = 750.0;
    const COLLAPSED_WIDTH: f64 = 460.0; // Left panel (450) + some padding
    const ANIMATION_DURATION: f64 = 0.25;

    unsafe {
        let mut state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
        state.sidecar_collapsed = !state.sidecar_collapsed;
        let is_collapsed = state.sidecar_collapsed;

        // Update button title
        let new_title = if is_collapsed { "<|" } else { ">|" };
        let title = ns_string(new_title);
        let _: () = msg_send![sender, setTitle: title];

        // Hide right panel elements BEFORE collapsing (so they're hidden during animation)
        if is_collapsed {
            if let Some(tab_ptr) = state.tab_bar {
                set_hidden(tab_ptr as Id, true);
            }
            if let Some(scroll_ptr) = state.drafts_scroll_view {
                set_hidden(scroll_ptr as Id, true);
            }
            if let Some(view_ptr) = state.voice_draft_view {
                set_hidden(view_ptr as Id, true);
            }
            if let Some(header_ptr) = state.voice_draft_header {
                set_hidden(header_ptr as Id, true);
            }
        }

        // Animate window width change (drawer slide)
        if let Some(window_ptr) = state.window {
            let window = window_ptr as Id;
            let target_width = if is_collapsed {
                COLLAPSED_WIDTH
            } else {
                EXPANDED_WIDTH
            };
            animate_window_width(window, target_width, ANIMATION_DURATION);
        }

        // Show right panel elements AFTER expanding (schedule after animation)
        if !is_collapsed {
            // Dispatch after animation completes
            let tab_ptr = state.tab_bar;
            let scroll_ptr = state.drafts_scroll_view;
            let voice_ptr = state.voice_draft_view;
            let header_ptr = state.voice_draft_header;

            dispatch::Queue::main().exec_after(
                std::time::Duration::from_millis((ANIMATION_DURATION * 1000.0) as u64 + 50),
                move || {
                    if let Some(ptr) = tab_ptr {
                        set_hidden(ptr as Id, false);
                    }
                    if let Some(ptr) = scroll_ptr {
                        set_hidden(ptr as Id, false);
                    }
                    if let Some(ptr) = voice_ptr {
                        set_hidden(ptr as Id, false);
                    }
                    if let Some(ptr) = header_ptr {
                        set_hidden(ptr as Id, false);
                    }
                },
            );
        }

        info!("Sidecar collapsed: {} (animated)", is_collapsed);
    }
}

extern "C" fn on_draft_edit(_this: &Object, _cmd: Sel, _sender: Id) {
    let state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(index) = state.selected_draft_index {
        if let Some(path) = state.draft_files.get(index) {
            let opened = open_file_in_editor(path);
            if opened {
                info!("Opened draft in editor: {}", path.display());
            } else {
                info!("Failed to open draft: {}", path.display());
            }
        }
    } else {
        info!("No draft selected for edit");
    }
}

extern "C" fn on_draft_copy(_this: &Object, _cmd: Sel, _sender: Id) {
    let state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(index) = state.selected_draft_index {
        if let Some(path) = state.draft_files.get(index) {
            if let Ok(content) = std::fs::read_to_string(path) {
                copy_to_clipboard(&content);
                info!("Copied draft to clipboard: {}", path.display());
            } else {
                info!("Failed to read draft: {}", path.display());
            }
        }
    } else {
        info!("No draft selected for copy");
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
// Settings Tab Handlers
// ═══════════════════════════════════════════════════════════

extern "C" fn on_settings_ai_formatting(_this: &Object, _cmd: Sel, sender: Id) {
    unsafe {
        let state_val: isize = msg_send![sender, state];
        let enabled = state_val == 1;

        // Save to config and sync tray menu
        let new_state = crate::tray::toggle_ai_formatting();
        // If state doesn't match what user clicked, toggle again
        if new_state != enabled {
            let _ = crate::tray::toggle_ai_formatting();
        }

        info!(
            "AI Formatting toggled via Settings: {}",
            if enabled { "ON" } else { "OFF" }
        );
    }
}

extern "C" fn on_settings_edit_config(_this: &Object, _cmd: Sel, _sender: Id) {
    // Open .env config file in default editor
    if let Some(base_dirs) = directories::BaseDirs::new() {
        let config_path = base_dirs.home_dir().join(".codescribe").join(".env");
        if config_path.exists() {
            let _ = std::process::Command::new("open")
                .arg("-t")
                .arg(&config_path)
                .spawn();
            info!("Opened config file: {}", config_path.display());
        } else {
            info!("Config file not found: {}", config_path.display());
        }
    }
}

extern "C" fn on_settings_edit_prompt(_this: &Object, _cmd: Sel, _sender: Id) {
    codescribe_core::config::prompts::open_prompt_file("formatting.txt");
    info!("Opened formatting prompt for editing");
}

extern "C" fn on_settings_open_prompts_folder(_this: &Object, _cmd: Sel, _sender: Id) {
    codescribe_core::config::prompts::open_prompts_folder();
    info!("Opened prompts folder");
}

extern "C" fn on_settings_reset_context(_this: &Object, _cmd: Sel, _sender: Id) {
    codescribe_core::state::conversation::reset_conversation();
    codescribe_core::ai_formatting::reset_ollama_memory();
    info!("AI context reset via Settings");
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
