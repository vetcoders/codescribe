//! Public API and internal helpers for voice chat overlay
//!
//! Contains all the public functions for controlling the overlay and
//! internal helper functions for state updates.

use core_graphics::geometry::CGPoint;
use dispatch::Queue;
use objc::runtime::{Class, Object};
use objc::{msg_send, sel, sel_impl};
use std::time::{Duration, SystemTime};
use tracing::{debug, info};

use crate::ui_helpers::{
    BubbleConfig, BubbleRole, animate_fade, create_bubble_view, create_card_view, list_draft_files,
    set_hidden, stack_view_add, stack_view_clear, update_bubble_text, window_close,
};

use super::state::{
    ChatMessage, ChatRole, DrawerEntry, OVERLAY_STATE, SEND_CALLBACK, Tab, TranscriptionMode,
    VoiceChatOverlayState,
};

// Type alias for Objective-C object pointers
// SAFETY: raw Objective-C pointers used in AppKit FFI.
type Id = *mut Object;

// ═══════════════════════════════════════════════════════════
// Public API
// ═══════════════════════════════════════════════════════════

/// Update the status text in the overlay
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

/// Submit the current draft (manual send)
pub fn send_voice_chat_draft() {
    Queue::main().exec_async(move || {
        send_draft_message_impl();
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

/// Clear the text content of the overlay
pub fn clear_voice_chat_text() {
    Queue::main().exec_async(|| {
        clear_voice_chat_text_impl();
    });
}

/// Check if auto-send is enabled
pub fn is_auto_send_enabled() -> bool {
    let state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
    state.auto_send_enabled
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

/// Reset the auto-hide timer (placeholder for future implementation)
pub fn reset_voice_chat_activity() {
    // Currently no auto-hide timer, but function exists for API compatibility
    debug!("reset_voice_chat_activity called");
}

/// Hide the voice chat overlay window
pub fn hide_voice_chat_overlay() {
    Queue::main().exec_async(|| {
        hide_voice_chat_overlay_impl();
    });
}

/// Switch the active tab in the overlay
pub fn set_active_tab(tab: Tab) {
    Queue::main().exec_async(move || {
        set_active_tab_impl(tab);
    });
}

/// Refresh drawer entries from disk
pub fn refresh_drawer() {
    Queue::main().exec_async(|| {
        refresh_drawer_impl();
    });
}

/// Filter drawer entries using a query
pub fn filter_drawer(query: &str) {
    let query_owned = query.to_string();
    Queue::main().exec_async(move || {
        filter_drawer_impl(&query_owned);
    });
}

// ═══════════════════════════════════════════════════════════
// Internal Implementation Functions
// ═══════════════════════════════════════════════════════════

fn update_voice_chat_status_impl(status: &str) {
    unsafe {
        let mut state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(title_ptr) = state.title_label {
            let title_label = title_ptr as Id;
            let display = format!("CodeScribe — {}", status);
            let ns_string_class = Class::get("NSString").unwrap();
            let mut c_str = display.as_bytes().to_vec();
            c_str.push(0);
            let ns_str: Id = msg_send![ns_string_class, stringWithUTF8String: c_str.as_ptr()];
            let _: () = msg_send![title_label, setStringValue: ns_str];
        }
    }
}

fn append_voice_chat_assistant_delta_impl(delta: &str) {
    let mut state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
    ensure_streaming_assistant_message(&mut state);
    if let Some(last) = state.messages.last_mut() {
        last.text.push_str(delta);
        last.is_streaming = true;
    }
    update_chat_view_with_state(&mut state, false);
}

fn clear_voice_chat_text_impl() {
    let mut state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
    state.messages.clear();
    state.manual_draft.clear();
    state.is_sending = false;
    update_chat_view_with_state(&mut state, true);
    update_input_field_with_state(&mut state);
    update_send_button_with_state(&mut state);
}

fn hide_voice_chat_overlay_impl() {
    unsafe {
        let mut state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(window_ptr) = state.window.take() {
            let window = window_ptr as Id;
            animate_fade(window, 0.0, 0.15);
            window_close(window);
            debug!("Voice chat overlay hidden");
        }
        clear_overlay_state(&mut state);
    }
}

fn ensure_streaming_assistant_message(state: &mut VoiceChatOverlayState) {
    let needs_new = match state.messages.last() {
        Some(last) => last.role != ChatRole::Assistant || !last.is_streaming,
        None => true,
    };
    if needs_new {
        state.messages.push(ChatMessage {
            role: ChatRole::Assistant,
            text: String::new(),
            is_streaming: true,
            is_error: false,
        });
    }
}

pub fn finalize_assistant_message_impl(text: &str, is_error: bool) {
    let mut state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
    let needs_new = match state.messages.last_mut() {
        Some(last) if last.role == ChatRole::Assistant => {
            last.text = text.to_string();
            last.is_streaming = false;
            last.is_error = is_error;
            false
        }
        _ => true,
    };
    if needs_new {
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

/// Send the draft message (called from handlers)
pub fn send_draft_message_impl() {
    let callback = {
        let mut state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
        let draft = state.manual_draft.trim().to_string();
        if draft.is_empty() {
            return;
        }
        state.messages.push(ChatMessage {
            role: ChatRole::User,
            text: draft.clone(),
            is_streaming: false,
            is_error: false,
        });
        state.manual_draft.clear();
        state.is_sending = true;
        update_chat_view_with_state(&mut state, true);
        update_input_field_with_state(&mut state);
        update_send_button_with_state(&mut state);
        let handler = SEND_CALLBACK.lock().unwrap_or_else(|e| e.into_inner());
        (handler.clone(), draft)
    };

    if let (Some(handler), draft) = callback {
        handler(draft);
    } else {
        let mut state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
        state.is_sending = false;
        update_send_button_with_state(&mut state);
    }
}

pub(crate) fn set_active_tab_impl(tab: Tab) {
    let mut state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
    state.active_tab = tab;

    if let Some(tab_control_ptr) = state.tab_control {
        unsafe {
            let control = tab_control_ptr as Id;
            let selected = match tab {
                Tab::Drawer => 0,
                Tab::Agent => 1,
            };
            let _: () = msg_send![control, setSelectedSegment: selected];
        }
    }

    if let Some(drawer_ptr) = state.drawer_scroll_view {
        unsafe {
            set_hidden(drawer_ptr as Id, !matches!(tab, Tab::Drawer));
        }
    }
    if let Some(search_ptr) = state.search_field {
        unsafe {
            set_hidden(search_ptr as Id, !matches!(tab, Tab::Drawer));
        }
    }
    if let Some(agent_ptr) = state.agent_scroll_view {
        unsafe {
            set_hidden(agent_ptr as Id, !matches!(tab, Tab::Agent));
        }
    }
    if let Some(input_ptr) = state.agent_input_field {
        unsafe {
            set_hidden(input_ptr as Id, !matches!(tab, Tab::Agent));
        }
    }
    if let Some(send_ptr) = state.agent_send_button {
        unsafe {
            set_hidden(send_ptr as Id, !matches!(tab, Tab::Agent));
        }
    }
}

pub(crate) fn refresh_drawer_impl() {
    let mut state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
    state.drawer_entries = load_drawer_entries();
    render_drawer_entries(&mut state, None);
}

fn filter_drawer_impl(query: &str) {
    let mut state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
    let query = query.trim().to_lowercase();
    let filter = if query.is_empty() { None } else { Some(query) };
    render_drawer_entries(&mut state, filter.as_deref());
}

fn render_drawer_entries(state: &mut VoiceChatOverlayState, query: Option<&str>) {
    let Some(container_ptr) = state.drawer_container else {
        return;
    };
    let container = container_ptr as Id;

    unsafe {
        stack_view_clear(container);
    }

    let action_handler = state.action_handler.map(|ptr| ptr as Id);

    for (index, entry) in state.drawer_entries.iter().enumerate() {
        if let Some(query) = query {
            let haystack = format!(
                "{} {}",
                entry.preview.to_lowercase(),
                entry.path.to_string_lossy().to_lowercase()
            );
            if !haystack.contains(query) {
                continue;
            }
        }

        let card = create_drawer_card(entry, index, action_handler);
        unsafe {
            stack_view_add(container, card);
        }
    }
}

pub fn load_drawer_entries() -> Vec<DrawerEntry> {
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
    let today = chrono::Local::now().format("%Y-%m-%d").to_string();
    let dir = std::path::PathBuf::from(home)
        .join(".codescribe/transcriptions")
        .join(today);

    let mut entries = Vec::new();

    let file_paths = list_draft_files(&dir);
    for path in file_paths {
        let Ok(metadata) = std::fs::metadata(&path) else {
            continue;
        };
        let timestamp = metadata.modified().unwrap_or(SystemTime::now());
        let preview = std::fs::read_to_string(&path)
            .ok()
            .map(|content| {
                let trimmed = content.trim();
                trimmed.chars().take(120).collect::<String>()
            })
            .unwrap_or_else(|| String::from("(empty)"));

        entries.push(DrawerEntry {
            path,
            timestamp,
            mode: TranscriptionMode::Toggle,
            preview,
            is_ai_formatted: false,
            is_favorite: false,
        });
    }

    entries.sort_by(|a, b| b.timestamp.cmp(&a.timestamp));
    entries
}

pub fn create_drawer_card(entry: &DrawerEntry, index: usize, target: Option<Id>) -> Id {
    use core_graphics::geometry::{CGRect, CGSize};

    let card_width = 420.0;
    let card_height = 120.0;
    let frame = CGRect::new(
        &CGPoint::new(0.0, 0.0),
        &CGSize::new(card_width, card_height),
    );

    let title = if entry.is_ai_formatted { "✨" } else { "Tt" };
    let subtitle = format_time(entry.timestamp);
    let preview = entry.preview.clone();

    let card = create_card_view(frame, title, &subtitle, &preview);

    unsafe {
        let mode_label = match entry.mode {
            TranscriptionMode::Hold => "Ctrl+Hold",
            TranscriptionMode::Assistive => "Ctrl+Shift",
            TranscriptionMode::Toggle => "Toggle",
        };

        let mode_frame = CGRect::new(&CGPoint::new(12.0, 6.0), &CGSize::new(120.0, 14.0));
        let mode_view = crate::ui_helpers::label_sized(mode_frame, mode_label, 10.0, false);
        let _: () = msg_send![mode_view, setTextColor: crate::ui_helpers::color_white(0.6)];
        crate::ui_helpers::add_subview(card, mode_view);

        let path_frame = CGRect::new(
            &CGPoint::new(140.0, 6.0),
            &CGSize::new(card_width - 220.0, 14.0),
        );
        let path_text = entry.path.to_string_lossy();
        let path_view = crate::ui_helpers::label_sized(path_frame, &path_text, 9.0, false);
        let _: () = msg_send![path_view, setTextColor: crate::ui_helpers::color_white(0.4)];
        crate::ui_helpers::add_subview(card, path_view);

        let button_y = 6.0;
        let button_width = 50.0;
        let spacing = 6.0;
        let buttons = [
            ("Copy", sel!(onCardCopy:)),
            ("Edit", sel!(onCardEdit:)),
            ("Delete", sel!(onCardDelete:)),
        ];

        for (i, (label, action)) in buttons.iter().enumerate() {
            let x = card_width - (button_width + spacing) * (buttons.len() - i) as f64 - 8.0;
            let btn_frame =
                CGRect::new(&CGPoint::new(x, button_y), &CGSize::new(button_width, 18.0));
            let btn = crate::ui_helpers::create_button(
                btn_frame,
                label,
                crate::ui_helpers::button_style::INLINE,
            );
            let _: () = msg_send![btn, setTag: index as isize];
            if let Some(target) = target {
                let _: () = msg_send![btn, setTarget: target];
                let _: () = msg_send![btn, setAction: *action];
            }
            crate::ui_helpers::add_subview(card, btn);
        }

        let fav_frame = CGRect::new(
            &CGPoint::new(card_width - 28.0, card_height - 24.0),
            &CGSize::new(18.0, 16.0),
        );
        let fav_title = if entry.is_favorite { "♥" } else { "♡" };
        let fav_btn = crate::ui_helpers::create_button(
            fav_frame,
            fav_title,
            crate::ui_helpers::button_style::INLINE,
        );
        let _: () = msg_send![fav_btn, setTag: index as isize];
        if let Some(target) = target {
            let _: () = msg_send![fav_btn, setTarget: target];
            let _: () = msg_send![fav_btn, setAction: sel!(onCardFavorite:)];
        }
        crate::ui_helpers::add_subview(card, fav_btn);
    }

    card
}

fn format_time(timestamp: SystemTime) -> String {
    let now = SystemTime::now();
    let diff = now
        .duration_since(timestamp)
        .unwrap_or(Duration::from_secs(0));
    if diff < Duration::from_secs(3600) {
        let mins = diff.as_secs() / 60;
        if mins == 0 {
            "just now".to_string()
        } else {
            format!("{} min ago", mins)
        }
    } else if diff < Duration::from_secs(86400) {
        let hours = diff.as_secs() / 3600;
        format!("{} hr ago", hours)
    } else {
        let dt: chrono::DateTime<chrono::Local> = timestamp.into();
        dt.format("%H:%M").to_string()
    }
}

// ═══════════════════════════════════════════════════════════
// UI State Update Functions (used by handlers and mod.rs)
// ═══════════════════════════════════════════════════════════

pub fn update_chat_view_with_state(state: &mut VoiceChatOverlayState, force_rebuild: bool) {
    let Some(container_ptr) = state.agent_container else {
        return;
    };
    let container = container_ptr as Id;

    // Check if we need to rebuild bubbles or just update the last one
    let bubble_count = state.agent_bubble_views.len();
    let message_count = state.messages.len();

    if !force_rebuild
        && bubble_count == message_count
        && message_count > 0
        && let Some(last_msg) = state.messages.last()
        && last_msg.is_streaming
        && let Some((_, text_label_ptr)) = state.agent_bubble_views.last()
    {
        let text_label = *text_label_ptr as Id;
        unsafe {
            update_bubble_text(text_label, &last_msg.text, true);
        }
        return;
    }

    unsafe {
        stack_view_clear(container);
    }
    state.agent_bubble_views.clear();

    let max_bubble_width = 360.0;
    let action_handler = state.action_handler.map(|ptr| ptr as Id);

    let messages_count = state.messages.len();
    for (rev_idx, message) in state.messages.iter().rev().enumerate() {
        let original_idx = messages_count - 1 - rev_idx;

        let role = match message.role {
            ChatRole::User => BubbleRole::User,
            ChatRole::Assistant => BubbleRole::Assistant,
            ChatRole::System => BubbleRole::System,
        };

        let (message_index, copy_target) = if !message.is_streaming {
            (Some(original_idx), action_handler)
        } else {
            (None, None)
        };

        let config = BubbleConfig {
            text: message.text.clone(),
            role,
            max_width: max_bubble_width,
            is_streaming: message.is_streaming,
            is_error: message.is_error,
            message_index,
            copy_action_target: copy_target,
        };

        let (bubble_view, text_label) = create_bubble_view(config);
        unsafe {
            stack_view_add(container, bubble_view);
        }
        state
            .agent_bubble_views
            .push((bubble_view as usize, text_label as usize));
    }

    if force_rebuild && let Some(scroll_view_ptr) = state.agent_scroll_view {
        unsafe {
            let scroll_view = scroll_view_ptr as Id;
            let content_view: Id = msg_send![scroll_view, contentView];
            let _: () = msg_send![content_view, scrollToPoint: CGPoint { x: 0.0, y: 0.0 }];
            let _: () = msg_send![scroll_view, reflectScrolledClipView: content_view];
        }
    }
}

pub fn update_input_field_with_state(state: &mut VoiceChatOverlayState) {
    unsafe {
        if let Some(input_ptr) = state.agent_input_field {
            let input_field = input_ptr as Id;
            let ns_string_class = Class::get("NSString").unwrap();
            let mut c_str = state.manual_draft.as_bytes().to_vec();
            c_str.push(0);
            let ns_str: Id = msg_send![ns_string_class, stringWithUTF8String: c_str.as_ptr()];
            let _: () = msg_send![input_field, setStringValue: ns_str];
        }
    }
}

pub fn update_send_button_with_state(state: &mut VoiceChatOverlayState) {
    unsafe {
        if let Some(send_ptr) = state.agent_send_button {
            let send_button = send_ptr as Id;
            let enabled = !state.is_sending && !state.manual_draft.trim().is_empty();
            let _: () = msg_send![send_button, setEnabled: enabled];
        }
    }
}

/// Clear all overlay state (called when window closes)
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
    state.drawer_entries.clear();
    state.search_field = None;
    state.agent_scroll_view = None;
    state.agent_container = None;
    state.agent_bubble_views.clear();
    state.agent_input_field = None;
    state.agent_send_button = None;
    state.messages.clear();
    state.manual_draft.clear();
    state.is_sending = false;
}
