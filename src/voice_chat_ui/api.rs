//! Public API and internal helpers for voice chat overlay
//!
//! Contains all the public functions for controlling the overlay and
//! internal helper functions for state updates.

use chrono::Local;
use core_graphics::geometry::{CGPoint, CGRect, CGSize};
use dispatch::Queue;
use objc::runtime::Object;
use objc::{msg_send, sel, sel_impl};


use crate::ui_helpers::{
    BubbleConfig, BubbleRole, button_set_action, create_bubble_view, create_button,
    create_card_view, list_draft_files, ns_string, open_file_in_editor, set_hidden, set_text,
    stack_view_add, stack_view_clear,
};

use super::handlers::copy_to_clipboard;
use super::state::{
    ChatMessage, ChatRole, DrawerEntry, OVERLAY_STATE, SEND_CALLBACK, Tab, TranscriptionMode,
    VoiceChatOverlayState,
};

// Type alias for Objective-C object pointers

type Id = *mut Object;

// ═══════════════════════════════════════════════════════════
// Public API
// ═══════════════════════════════════════════════════════════

/// Update the status text in the overlay header
pub fn update_voice_chat_status(status: &str) {
    let status_owned = status.to_string();
    Queue::main().exec_async(move || {
        update_voice_chat_status_impl(&status_owned);
    });
}

/// Append a delta to the assistant response (streaming)
pub fn append_voice_chat_assistant_delta(delta: &str) {
    let delta_owned = delta.to_string();
    Queue::main().exec_async(move || {
        append_voice_chat_assistant_delta_impl(&delta_owned);
    });
}

/// Set the full text in the overlay for the assistant response
pub fn set_voice_chat_text(text: &str) {
    let text_owned = text.to_string();
    Queue::main().exec_async(move || {
        finalize_assistant_message_impl(&text_owned, false);
    });
}

/// Add an error message to the chat log
pub fn add_voice_chat_error_message(text: &str) {
    let text_owned = text.to_string();
    Queue::main().exec_async(move || {
        let mut state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
        state.messages.push(ChatMessage {
            role: ChatRole::System,
            text: text_owned.clone(),
            is_streaming: false,
            is_error: true,
        });
        state.is_sending = false;
        update_chat_view_with_state(&mut state, true);
        update_send_button_with_state(&mut state);
    });
}

/// Set the send callback invoked when the user submits a message
pub fn set_voice_chat_send_callback(callback: Option<super::state::VoiceChatSendCallback>) {
    let mut handler = SEND_CALLBACK.lock().unwrap_or_else(|e| e.into_inner());
    *handler = callback;
}

/// Toggle loading state for sending
pub fn set_voice_chat_sending(is_sending: bool) {
    Queue::main().exec_async(move || {
        let mut state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
        state.is_sending = is_sending;
        update_send_button_with_state(&mut state);
    });
}

/// Add a user message to the chat
pub fn add_voice_chat_user_message(text: &str) {
    let text_owned = text.to_string();
    Queue::main().exec_async(move || {
        let mut state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
        state.messages.push(ChatMessage {
            role: ChatRole::User,
            text: text_owned,
            is_streaming: false,
            is_error: false,
        });
        update_chat_view_with_state(&mut state, true);
    });
}

/// Check if the voice chat overlay is currently visible
pub fn is_voice_chat_overlay_visible() -> bool {
    let state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
    state.window.is_some()
}

/// Hide the voice chat overlay window
pub fn hide_voice_chat_overlay() {
    Queue::main().exec_async(|| {
        hide_voice_chat_overlay_impl();
    });
}

/// Load drawer entries from today
pub fn load_drawer_entries() -> Vec<DrawerEntry> {
    let mut entries = Vec::new();
    let Some(base_dirs) = directories::BaseDirs::new() else {
        return entries;
    };
    let today = Local::now().format("%Y-%m-%d").to_string();
    let dir = base_dirs
        .home_dir()
        .join(".codescribe")
        .join("transcriptions")
        .join(today);

    for path in list_draft_files(&dir) {
        let timestamp = std::fs::metadata(&path)
            .and_then(|meta| meta.modified())
            .unwrap_or_else(|_| std::time::SystemTime::now());
        let preview = std::fs::read_to_string(&path)
            .unwrap_or_default()
            .chars()
            .take(120)
            .collect::<String>();
        entries.push(DrawerEntry {
            path,
            timestamp,
            mode: TranscriptionMode::Toggle,
            preview,
            is_ai_formatted: false,
            is_favorite: false,
        });
    }

    entries
}

/// Refresh drawer list
pub fn refresh_drawer() {
    Queue::main().exec_async(|| {
        let mut state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
        state.drawer_entries = load_drawer_entries();
        rebuild_drawer(&mut state, None);
    });
}

/// Filter drawer entries by query
pub fn filter_drawer(query: &str) {
    let query_owned = query.to_string();
    Queue::main().exec_async(move || {
        let mut state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
        state.drawer_entries = load_drawer_entries();
        rebuild_drawer(&mut state, Some(&query_owned));
    });
}

/// Set active tab and update view visibility
pub fn set_active_tab(tab: Tab) {
    let mut state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
    state.active_tab = tab;
    if let Some(tab_control_ptr) = state.tab_control {
        unsafe {
            let tab_control = tab_control_ptr as Id;
            let selected: isize = if tab == Tab::Drawer { 0 } else { 1 };
            let _: () = msg_send![tab_control, setSelectedSegment: selected];
        }
    }

    if let Some(drawer_ptr) = state.drawer_scroll_view {
        unsafe { set_hidden(drawer_ptr as Id, tab != Tab::Drawer) };
    }
    if let Some(search_ptr) = state.search_field {
        unsafe { set_hidden(search_ptr as Id, tab != Tab::Drawer) };
    }
    if let Some(agent_ptr) = state.agent_scroll_view {
        unsafe { set_hidden(agent_ptr as Id, tab != Tab::Agent) };
    }
    if let Some(input_ptr) = state.agent_input_field {
        unsafe { set_hidden(input_ptr as Id, tab != Tab::Agent) };
    }
    if let Some(button_ptr) = state.agent_send_button {
        unsafe { set_hidden(button_ptr as Id, tab != Tab::Agent) };
    }
}

/// Switch to agent tab
pub fn show_agent_tab() {
    set_active_tab(Tab::Agent);
}

// ═══════════════════════════════════════════════════════════
// Internal Implementation Functions
// ═══════════════════════════════════════════════════════════

fn update_voice_chat_status_impl(status: &str) {
    unsafe {
        let state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(title_ptr) = state.title_label {
            let title = if status.is_empty() {
                "CodeScribe".to_string()
            } else {
                format!("CodeScribe — {}", status)
            };
            let title_label = title_ptr as Id;
            let _: () = msg_send![title_label, setStringValue: ns_string(&title)];
        }
    }
}

fn append_voice_chat_assistant_delta_impl(delta: &str) {
    let mut state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(last) = state.messages.last_mut() {
        if last.role == ChatRole::Assistant && last.is_streaming {
            last.text.push_str(delta);
        } else {
            state.messages.push(ChatMessage {
                role: ChatRole::Assistant,
                text: delta.to_string(),
                is_streaming: true,
                is_error: false,
            });
        }
    } else {
        state.messages.push(ChatMessage {
            role: ChatRole::Assistant,
            text: delta.to_string(),
            is_streaming: true,
            is_error: false,
        });
    }

    update_chat_view_with_state(&mut state, true);
}

fn finalize_assistant_message_impl(text: &str, is_error: bool) {
    let mut state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(last) = state.messages.last_mut() {
        if last.role == ChatRole::Assistant {
            last.text = text.to_string();
            last.is_streaming = false;
            last.is_error = is_error;
        } else {
            state.messages.push(ChatMessage {
                role: ChatRole::Assistant,
                text: text.to_string(),
                is_streaming: false,
                is_error,
            });
        }
    } else {
        state.messages.push(ChatMessage {
            role: ChatRole::Assistant,
            text: text.to_string(),
            is_streaming: false,
            is_error,
        });
    }
    state.is_sending = false;
    update_chat_view_with_state(&mut state, true);
    update_send_button_with_state(&mut state);
}

pub fn update_chat_view_with_state(state: &mut VoiceChatOverlayState, scroll_to_bottom: bool) {
    unsafe {
        let Some(container_ptr) = state.agent_container else {
            return;
        };
        let container = container_ptr as Id;
        stack_view_clear(container);
        state.agent_bubble_views.clear();

        let max_width = 420.0;
        for (index, message) in state.messages.iter().enumerate() {
            let role = match message.role {
                ChatRole::User => BubbleRole::User,
                ChatRole::Assistant => BubbleRole::Assistant,
                ChatRole::System => BubbleRole::System,
            };
            let (container_view, text_label) = create_bubble_view(BubbleConfig {
                text: message.text.clone(),
                role,
                max_width,
                is_streaming: message.is_streaming,
                is_error: message.is_error,
                message_index: if message.role == ChatRole::Assistant {
                    Some(index)
                } else {
                    None
                },
                copy_action_target: state.action_handler.map(|v| v as Id),
            });
            stack_view_add(container, container_view);
            state
                .agent_bubble_views
                .push((container_view as usize, text_label as usize));
        }

        if scroll_to_bottom {
            if let Some(scroll_ptr) = state.agent_scroll_view {
                let scroll_view = scroll_ptr as Id;
                let content_view: Id = msg_send![scroll_view, contentView];
                let document_view: Id = msg_send![scroll_view, documentView];
                let doc_frame: CGRect = msg_send![document_view, frame];
                let new_origin = CGPoint::new(0.0, doc_frame.size.height);
                let _: () = msg_send![content_view, scrollToPoint: new_origin];
            }
        }
    }
}

pub fn update_input_field_with_state(state: &VoiceChatOverlayState) {
    if let Some(input_ptr) = state.agent_input_field {
        unsafe { set_text(input_ptr as Id, &state.manual_draft) };
    }
}

pub fn update_send_button_with_state(state: &VoiceChatOverlayState) {
    if let Some(send_ptr) = state.agent_send_button {
        unsafe {
            let send_btn = send_ptr as Id;
            let title = if state.is_sending { "…" } else { ">" };
            let _: () = msg_send![send_btn, setTitle: ns_string(title)];
        }
    }
}

pub fn send_draft_message_impl() {
    unsafe {
        let mut state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
        let Some(input_ptr) = state.agent_input_field else {
            return;
        };
        let input = get_input_value(input_ptr as Id);
        let trimmed = input.trim();
        if trimmed.is_empty() {
            return;
        }
        let message = trimmed.to_string();
        state.manual_draft.clear();
        set_text(input_ptr as Id, "");

        state.messages.push(ChatMessage {
            role: ChatRole::User,
            text: message.clone(),
            is_streaming: false,
            is_error: false,
        });
        update_chat_view_with_state(&mut state, true);
        update_send_button_with_state(&mut state);

        if let Some(callback) = SEND_CALLBACK.lock().unwrap_or_else(|e| e.into_inner()).clone() {
            callback(message);
        }
    }
}

pub fn clear_overlay_state(state: &mut VoiceChatOverlayState) {
    state.window = None;
    state.window_delegate = None;
    state.blur_view = None;
    state.title_label = None;
    state.tab_control = None;
    state.close_button = None;
    state.settings_button = None;
    state.drawer_scroll_view = None;
    state.drawer_container = None;
    state.search_field = None;
    state.agent_scroll_view = None;
    state.agent_container = None;
    state.agent_bubble_views.clear();
    state.agent_input_field = None;
    state.agent_send_button = None;
    state.action_handler = None;
}

fn rebuild_drawer(state: &mut VoiceChatOverlayState, query: Option<&str>) {
    unsafe {
        let Some(container_ptr) = state.drawer_container else {
            return;
        };
        let container = container_ptr as Id;
        stack_view_clear(container);

        let normalized_query = query.map(|q| q.to_lowercase());
        for (index, entry) in state.drawer_entries.iter().enumerate() {
            if let Some(ref q) = normalized_query {
                let haystack = entry.preview.to_lowercase();
                if !haystack.contains(q) {
                    continue;
                }
            }
            let card_view = create_drawer_card(entry, index, state.action_handler);
            stack_view_add(container, card_view);
        }
    }
}

fn create_drawer_card(entry: &DrawerEntry, index: usize, target: Option<usize>) -> Id {
    unsafe {
        let card_height = 110.0;
        let card_frame = CGRect::new(&CGPoint::new(0.0, 0.0), &CGSize::new(420.0, card_height));
        let time_label = format_time_label(entry.timestamp);
        let title = format!("Tt   {}", time_label);
        let subtitle = entry
            .path
            .to_string_lossy()
            .split("/home/")
            .last()
            .unwrap_or_else(|| entry.path.to_string_lossy().as_ref())
            .to_string();
        let preview = entry.preview.clone();
        let card = create_card_view(card_frame, &title, &subtitle, &preview);

        let button_y = 12.0;
        let button_width = 52.0;
        let button_height = 20.0;
        let padding = 12.0;
        let button_spacing = 6.0;
        let mut x = padding;

        let target = target.map(|v| v as Id);

        if let Some(target) = target {
            let copy_btn = create_button(
                CGRect::new(&CGPoint::new(x, button_y), &CGSize::new(button_width, button_height)),
                "Copy",
                15,
            );
            let _: () = msg_send![copy_btn, setTag: index as isize];
            button_set_action(copy_btn, target, sel!(onCardCopy:));
            let _: () = msg_send![card, addSubview: copy_btn];
            x += button_width + button_spacing;

            let edit_btn = create_button(
                CGRect::new(&CGPoint::new(x, button_y), &CGSize::new(button_width, button_height)),
                "Edit",
                15,
            );
            let _: () = msg_send![edit_btn, setTag: index as isize];
            button_set_action(edit_btn, target, sel!(onCardEdit:));
            let _: () = msg_send![card, addSubview: edit_btn];
            x += button_width + button_spacing;

            let delete_btn = create_button(
                CGRect::new(&CGPoint::new(x, button_y), &CGSize::new(button_width, button_height)),
                "Delete",
                15,
            );
            let _: () = msg_send![delete_btn, setTag: index as isize];
            button_set_action(delete_btn, target, sel!(onCardDelete:));
            let _: () = msg_send![card, addSubview: delete_btn];

            let fav_btn = create_button(
                CGRect::new(
                    &CGPoint::new(card_frame.size.width - 40.0, card_height - 24.0),
                    &CGSize::new(24.0, 18.0),
                ),
                if entry.is_favorite { "♥" } else { "♡" },
                15,
            );
            let _: () = msg_send![fav_btn, setTag: index as isize];
            button_set_action(fav_btn, target, sel!(onCardFavorite:));
            let _: () = msg_send![card, addSubview: fav_btn];
        }

        card
    }
}

fn format_time_label(timestamp: std::time::SystemTime) -> String {
    let datetime: chrono::DateTime<chrono::Local> = timestamp.into();
    datetime.format("%H:%M").to_string()
}

fn get_input_value(input: Id) -> String {
    unsafe {
        let text: Id = msg_send![input, stringValue];
        let c_str: *const i8 = msg_send![text, UTF8String];
        if c_str.is_null() {
            return String::new();
        }
        let rust_string = std::ffi::CStr::from_ptr(c_str)
            .to_string_lossy()
            .to_string();
        let _: () = msg_send![input, setStringValue: ns_string("")];
        rust_string
    }
}

fn hide_voice_chat_overlay_impl() {
    unsafe {
        let mut state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(window_ptr) = state.window {
            let window = window_ptr as Id;
            let nil: Id = std::ptr::null_mut();
            let _: () = msg_send![window, orderOut: nil];
        }
        clear_overlay_state(&mut state);
    }
}

pub fn open_drawer_entry_in_editor(index: usize) {
    let state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(entry) = state.drawer_entries.get(index) {
        let _ = open_file_in_editor(&entry.path);
    }
}

pub fn copy_drawer_entry(index: usize) {
    let state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(entry) = state.drawer_entries.get(index) {
        if let Ok(contents) = std::fs::read_to_string(&entry.path) {
            copy_to_clipboard(&contents);
        }
    }
}

pub fn delete_drawer_entry(index: usize) {
    let entry = {
        let state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
        state.drawer_entries.get(index).cloned()
    };
    if let Some(entry) = entry {
        if std::fs::remove_file(&entry.path).is_ok() {
            refresh_drawer();
        }
    }
}
