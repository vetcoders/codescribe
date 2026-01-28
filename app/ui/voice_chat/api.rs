//! Public API and internal helpers for voice chat overlay
//!
//! Contains all the public functions for controlling the overlay and
//! internal helper functions for state updates.

use core_graphics::geometry::{CGPoint, CGRect, CGSize};
use dispatch::Queue;
use objc::runtime::{Class, Object};
use objc::{msg_send, sel, sel_impl};
use std::time::SystemTime;
use tracing::{debug, info, warn};

use crate::ui_helpers::{
    BubbleConfig, BubbleRole, create_bubble_view, create_card_view, get_text_field_string,
    list_draft_files, ns_string, open_file_in_editor, set_text_field_string, stack_view_add,
    stack_view_clear, window_set_alpha, window_show,
};

use super::handlers::{clear_search_field, copy_to_clipboard};
use super::state::{
    ChatMessage, ChatRole, ConversationModeState, DrawerEntry, OVERLAY_STATE, SEND_CALLBACK, Tab,
    TranscriptionMode, VoiceChatOverlayState,
};

// Type alias for Objective-C object pointers
pub type Id = *mut Object;

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

/// Update the context summary shown in the overlay header (best-effort debug aid).
///
/// Examples:
/// - `ctx: Visual Studio Code | sel: 123`
/// - `ctx: Finder | sel: 0`
pub fn update_voice_chat_context_summary(summary: &str) {
    let summary_owned = summary.to_string();
    Queue::main().exec_async(move || {
        update_voice_chat_context_summary_impl(&summary_owned);
    });
}

/// Append a delta to the user draft message (streaming transcription)
pub fn append_voice_chat_user_delta(delta: &str) {
    let delta_owned = delta.to_string();
    Queue::main().exec_async(move || {
        append_voice_chat_user_delta_impl(&delta_owned);
    });
}

/// Finalize the user message text (stop streaming)
pub fn set_voice_chat_user_text(text: &str) {
    let text_owned = text.to_string();
    Queue::main().exec_async(move || {
        finalize_user_message_impl(&text_owned);
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

/// Submit the current draft (manual send)
pub fn send_voice_chat_draft() {
    Queue::main().exec_async(|| {
        send_draft_message_impl();
    });
}

/// Set the send callback invoked when the user submits a message
pub fn set_voice_chat_send_callback(callback: Option<super::state::VoiceChatSendCallback>) {
    let mut handler = SEND_CALLBACK.lock().unwrap_or_else(|e| e.into_inner());
    *handler = callback.clone();
    drop(handler);

    let mut state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
    state.auto_send_enabled = callback.is_some();
    update_send_button_with_state(&mut state);
}

/// Toggle loading state for sending
pub fn set_voice_chat_sending(is_sending: bool) {
    Queue::main().exec_async(move || {
        let mut state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
        state.is_sending = is_sending;
        update_send_button_with_state(&mut state);
    });
}

/// Clear all chat messages and reset input state
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

/// Check if the voice chat overlay is currently visible
pub fn is_voice_chat_overlay_visible() -> bool {
    let state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
    state.window.is_some()
}

/// Reset the auto-hide timer (placeholder for future implementation)
pub fn reset_voice_chat_activity() {
    debug!("reset_voice_chat_activity called");
}

/// Hide the voice chat overlay window
pub fn hide_voice_chat_overlay() {
    Queue::main().exec_async(|| {
        hide_voice_chat_overlay_impl();
    });
}

/// Refresh drawer entries from disk
pub fn refresh_drawer() {
    Queue::main().exec_async(|| {
        refresh_drawer_impl();
    });
}

/// Filter drawer entries by query (reloads from disk)
pub fn filter_drawer(query: &str) {
    let query_owned = query.to_string();
    Queue::main().exec_async(move || {
        let mut state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
        state.drawer_entries = load_drawer_entries();
        render_drawer_entries(&mut state, &query_owned);
    });
}

/// Switch to Agent tab programmatically
pub fn show_agent_tab() {
    Queue::main().exec_async(|| {
        update_active_tab_impl(Tab::Agent);
    });
}

/// Switch to Drawer tab programmatically
pub fn show_drawer_tab() {
    Queue::main().exec_async(|| {
        update_active_tab_impl(Tab::Drawer);
    });
}

/// Set the target app name to re-activate for paste actions.
///
/// This is best-effort and primarily used to paste assistant output back into
/// the app where the user was working before interacting with the overlay.
pub fn set_voice_chat_target_app(app_name: Option<String>) {
    Queue::main().exec_async(move || {
        let mut state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
        state.last_target_app = app_name;
    });
}

/// Update the conversation mode state (Moshi full-duplex indicators)
pub fn update_conversation_state(new_state: ConversationModeState) {
    Queue::main().exec_async(move || {
        let mut state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
        state.conversation_state = new_state;
        // Update status text based on conversation state
        let status = match new_state {
            ConversationModeState::Inactive => "Ready",
            ConversationModeState::Listening => "Listening...",
            ConversationModeState::UserSpeaking => "You're speaking...",
            ConversationModeState::Processing => "Processing...",
            ConversationModeState::AssistantSpeaking => "Moshi responding...",
            ConversationModeState::Interrupted => "Interrupted",
        };
        drop(state);
        update_voice_chat_status_impl(status);
    });
}

/// Check if conversation mode is active
pub fn is_conversation_active() -> bool {
    let state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
    !matches!(state.conversation_state, ConversationModeState::Inactive)
}

// ═══════════════════════════════════════════════════════════
// Internal Implementation Functions
// ═══════════════════════════════════════════════════════════

pub fn update_active_tab_impl(tab: Tab) {
    let mut state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
    update_active_tab_locked(&mut state, tab);
}

fn update_active_tab_locked(state: &mut VoiceChatOverlayState, tab: Tab) {
    unsafe {
        state.active_tab = tab;

        if let Some(tab_control) = state.tab_control {
            let _: () = msg_send![
                tab_control as Id,
                setSelectedSegment: if tab == Tab::Drawer { 0_isize } else { 1_isize }
            ];
        }

        let show_drawer = tab == Tab::Drawer;
        if let Some(drawer_view) = state.drawer_scroll_view {
            crate::ui_helpers::set_hidden(drawer_view as Id, !show_drawer);
        }
        if let Some(search_field) = state.search_field {
            crate::ui_helpers::set_hidden(search_field as Id, !show_drawer);
        }
        if let Some(agent_view) = state.agent_scroll_view {
            crate::ui_helpers::set_hidden(agent_view as Id, show_drawer);
        }
        if let Some(agent_input) = state.agent_input_field {
            let superview: Id = msg_send![agent_input as Id, superview];
            if !superview.is_null() {
                crate::ui_helpers::set_hidden(superview, show_drawer);
            } else {
                crate::ui_helpers::set_hidden(agent_input as Id, show_drawer);
            }
        }
        if let Some(agent_send) = state.agent_send_button {
            crate::ui_helpers::set_hidden(agent_send as Id, show_drawer);
        }
    }
}

fn update_voice_chat_status_impl(status: &str) {
    unsafe {
        let mut state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
        state.status_text = status.to_string();
        if let Some(title_label) = state.title_label {
            let title = if state.context_text.trim().is_empty() {
                format!("CodeScribe — {}", state.status_text)
            } else {
                format!(
                    "CodeScribe — {} [{}]",
                    state.status_text,
                    state.context_text.trim()
                )
            };
            let ns_str = ns_string(&title);
            let _: () = msg_send![title_label as Id, setStringValue: ns_str];
        }
    }
}

fn update_voice_chat_context_summary_impl(summary: &str) {
    unsafe {
        let mut state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
        state.context_text = summary.to_string();

        if let Some(title_label) = state.title_label {
            let title = if state.context_text.trim().is_empty() {
                format!("CodeScribe — {}", state.status_text)
            } else {
                format!(
                    "CodeScribe — {} [{}]",
                    state.status_text,
                    state.context_text.trim()
                )
            };
            let ns_str = ns_string(&title);
            let _: () = msg_send![title_label as Id, setStringValue: ns_str];
        }
    }
}

fn append_voice_chat_user_delta_impl(delta: &str) {
    let mut state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
    ensure_agent_tab_visible(&mut state);
    ensure_streaming_user_message(&mut state);
    if let Some(last) = state.messages.last_mut() {
        apply_delta_with_backspace(&mut last.text, delta);
        last.is_streaming = true;
    }
    // Streaming deltas can arrive very frequently; avoid rebuilding the entire chat view
    // (AppKit object churn can spike CPU/RAM and even hard-freeze).
    if !try_update_last_message_view_in_place(&mut state) {
        update_chat_view_with_state(&mut state, false);
    }
}

fn append_voice_chat_assistant_delta_impl(delta: &str) {
    let mut state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
    ensure_agent_tab_visible(&mut state);
    ensure_streaming_assistant_message(&mut state);
    if let Some(last) = state.messages.last_mut() {
        apply_delta_with_backspace(&mut last.text, delta);
        last.is_streaming = true;
    }
    if !try_update_last_message_view_in_place(&mut state) {
        update_chat_view_with_state(&mut state, false);
    }
}

fn display_text_for_message(message: &ChatMessage) -> String {
    if message.is_streaming && message.text.is_empty() {
        "• • •".to_string()
    } else if message.is_streaming {
        format!("{} …", message.text)
    } else {
        message.text.clone()
    }
}

fn try_update_last_message_view_in_place(state: &mut VoiceChatOverlayState) -> bool {
    unsafe {
        // If the view list doesn't match messages, a full rebuild is safer.
        if state.agent_bubble_views.len() != state.messages.len() {
            return false;
        }

        let Some(last_message) = state.messages.last() else {
            return false;
        };
        let Some((_bubble_ptr, label_ptr)) = state.agent_bubble_views.last().copied() else {
            return false;
        };

        let label = label_ptr as Id;
        let display_text = display_text_for_message(last_message);
        set_text_field_string(label, &display_text);
        true
    }
}

fn apply_delta_with_backspace(target: &mut String, delta: &str) {
    for ch in delta.chars() {
        if ch == '\u{0008}' {
            target.pop();
        } else {
            target.push(ch);
        }
    }
}

fn finalize_user_message_impl(text: &str) {
    let mut state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
    ensure_agent_tab_visible(&mut state);
    ensure_streaming_user_message(&mut state);
    if let Some(last) = state.messages.last_mut() {
        last.text = text.to_string();
        last.is_streaming = false;
        last.is_error = false;
    }
    update_chat_view_with_state(&mut state, true);
}

fn finalize_assistant_message_impl(text: &str, is_error: bool) {
    let mut state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
    ensure_agent_tab_visible(&mut state);
    ensure_streaming_assistant_message(&mut state);
    if let Some(last) = state.messages.last_mut() {
        last.text = text.to_string();
        last.is_streaming = false;
        last.is_error = is_error;
    }
    state.is_sending = false;
    update_chat_view_with_state(&mut state, true);
    update_send_button_with_state(&mut state);
}

fn ensure_agent_tab_visible(state: &mut VoiceChatOverlayState) {
    unsafe {
        // Make sure the window is actually visible, even if it was previously hidden/closed.
        if let Some(window_ptr) = state.window {
            let window = window_ptr as Id;
            window_set_alpha(window, 1.0);
            window_show(window);
        }

        // Force Agent tab for any live/assistive messaging.
        state.active_tab = Tab::Agent;
        if let Some(tab_control) = state.tab_control {
            let _: () = msg_send![tab_control as Id, setSelectedSegment: 1_isize];
        }

        let show_drawer = false;
        if let Some(drawer_view) = state.drawer_scroll_view {
            crate::ui_helpers::set_hidden(drawer_view as Id, !show_drawer);
        }
        if let Some(search_field) = state.search_field {
            crate::ui_helpers::set_hidden(search_field as Id, !show_drawer);
        }
        if let Some(agent_view) = state.agent_scroll_view {
            crate::ui_helpers::set_hidden(agent_view as Id, show_drawer);
        }
        if let Some(agent_input) = state.agent_input_field {
            let superview: Id = msg_send![agent_input as Id, superview];
            if !superview.is_null() {
                crate::ui_helpers::set_hidden(superview, show_drawer);
            } else {
                crate::ui_helpers::set_hidden(agent_input as Id, show_drawer);
            }
        }
        if let Some(agent_send) = state.agent_send_button {
            crate::ui_helpers::set_hidden(agent_send as Id, show_drawer);
        }
    }
}

pub(super) fn clear_voice_chat_text_impl() {
    let mut state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
    state.messages.clear();
    state.manual_draft.clear();
    state.is_sending = false;

    if let Some(input_field) = state.agent_input_field {
        unsafe {
            set_text_field_string(input_field as Id, "");
        }
    }

    update_chat_view_with_state(&mut state, true);
    update_send_button_with_state(&mut state);
}

/// Send the draft message (called from handlers)
pub fn send_draft_message_impl() {
    let callback = {
        let mut state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
        let Some(input_field) = state.agent_input_field else {
            return;
        };
        let draft = unsafe { get_text_field_string(input_field as Id) };
        let draft = draft.trim().to_string();
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
        unsafe {
            set_text_field_string(input_field as Id, "");
        }
        update_chat_view_with_state(&mut state, true);
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

pub(super) fn commit_last_user_message_impl() {
    let callback = {
        let mut state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
        let Some(last_message) = state.messages.last() else {
            return;
        };
        if last_message.role != ChatRole::User {
            return;
        }
        let text = last_message.text.clone();
        state.is_sending = true;
        update_chat_view_with_state(&mut state, true);
        update_send_button_with_state(&mut state);
        let handler = SEND_CALLBACK.lock().unwrap_or_else(|e| e.into_inner());
        (handler.clone(), text)
    };

    if let (Some(handler), text) = callback {
        handler(text);
    } else {
        let mut state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
        state.is_sending = false;
        update_send_button_with_state(&mut state);
    }
}

pub(super) fn discard_last_message_impl() {
    let mut state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
    if state.messages.pop().is_some() {
        update_chat_view_with_state(&mut state, true);
    }
}

fn ensure_streaming_assistant_message(state: &mut VoiceChatOverlayState) {
    let needs_new = state
        .messages
        .last()
        .is_none_or(|msg| msg.role != ChatRole::Assistant || !msg.is_streaming);
    if needs_new {
        state.messages.push(ChatMessage {
            role: ChatRole::Assistant,
            text: String::new(),
            is_streaming: true,
            is_error: false,
        });
    }
}

fn ensure_streaming_user_message(state: &mut VoiceChatOverlayState) {
    let needs_new = state
        .messages
        .last()
        .is_none_or(|msg| msg.role != ChatRole::User || !msg.is_streaming);
    if needs_new {
        state.messages.push(ChatMessage {
            role: ChatRole::User,
            text: String::new(),
            is_streaming: true,
            is_error: false,
        });
    }
}

pub(super) fn update_chat_view_with_state(
    state: &mut VoiceChatOverlayState,
    scroll_to_bottom: bool,
) {
    unsafe {
        let Some(container_ptr) = state.agent_container else {
            return;
        };
        let container = container_ptr as Id;
        stack_view_clear(container);
        state.agent_bubble_views.clear();

        let mut last_bubble: Option<Id> = None;
        for (index, message) in state.messages.iter().enumerate() {
            let role = match message.role {
                ChatRole::User => BubbleRole::User,
                ChatRole::Assistant => BubbleRole::Assistant,
                ChatRole::System => BubbleRole::System,
            };
            let (bubble, text_label) = create_bubble_view(BubbleConfig {
                text: message.text.clone(),
                role,
                max_width: 390.0,
                is_streaming: message.is_streaming,
                is_error: message.is_error,
                message_index: Some(index),
                copy_action_target: state.action_handler.map(|p| p as Id),
            });
            stack_view_add(container, bubble);
            last_bubble = Some(bubble);
            state
                .agent_bubble_views
                .push((bubble as usize, text_label as usize));

            // Add commit/discard action bar for draft user messages
            if message.role == ChatRole::User
                && index == state.messages.len() - 1
                && !state.auto_send_enabled
                && !state.is_sending
            {
                let action_bar = create_commit_action_bar(state.action_handler);
                stack_view_add(container, action_bar);
            }
        }

        // Ensure the document view size matches its arranged subviews; otherwise scrolling can
        // jump to empty space and look like messages "disappear".
        if let Some(scroll_view_ptr) = state.agent_scroll_view {
            let scroll_view = scroll_view_ptr as Id;

            let _: () = msg_send![container, layoutSubtreeIfNeeded];
            let fitting: CGSize = msg_send![container, fittingSize];
            let frame: CGRect = msg_send![container, frame];
            let new_size = CGSize::new(frame.size.width, fitting.height.max(frame.size.height));
            let _: () = msg_send![container, setFrameSize: new_size];

            if scroll_to_bottom {
                // Prefer scrollRectToVisible over scrollToPoint (less sensitive to flipped coords).
                if let Some(bubble) = last_bubble {
                    let bounds: CGRect = msg_send![bubble, bounds];
                    let _: () = msg_send![bubble, scrollRectToVisible: bounds];
                } else {
                    let content_view: Id = msg_send![scroll_view, contentView];
                    let _: () = msg_send![content_view, scrollToPoint: CGPoint::new(0.0, 0.0)];
                    let _: () = msg_send![scroll_view, reflectScrolledClipView: content_view];
                }
            }
        }
    }
}

fn update_send_button_with_state(state: &mut VoiceChatOverlayState) {
    unsafe {
        if let Some(button_ptr) = state.agent_send_button {
            let btn = button_ptr as Id;
            let enabled = !state.is_sending && state.auto_send_enabled;
            let _: () = msg_send![btn, setEnabled: enabled];
            let title = if state.is_sending { "…" } else { ">" };
            let title = ns_string(title);
            let _: () = msg_send![btn, setTitle: title];
        }
    }
}

fn create_commit_action_bar(action_handler: Option<usize>) -> Id {
    unsafe {
        let ns_view = Class::get("NSView").unwrap();
        let max_width = 390.0;
        let bar_height = 28.0;

        let bar: Id = msg_send![ns_view, alloc];
        let bar_frame = core_graphics::geometry::CGRect::new(
            &CGPoint::new(0.0, 0.0),
            &core_graphics::geometry::CGSize::new(max_width, bar_height),
        );
        let bar: Id = msg_send![bar, initWithFrame: bar_frame];

        let btn_width = 64.0;
        let btn_height = 22.0;
        let gap = 8.0;
        let right_edge = max_width - 8.0;

        // Discard button (left of Commit)
        let discard_x = right_edge - btn_width * 2.0 - gap;
        let discard_btn = crate::ui_helpers::create_button(
            core_graphics::geometry::CGRect::new(
                &CGPoint::new(discard_x, 3.0),
                &core_graphics::geometry::CGSize::new(btn_width, btn_height),
            ),
            "Discard",
            crate::ui_helpers::button_style::SMALL_SQUARE,
        );
        if let Some(handler) = action_handler {
            crate::ui_helpers::button_set_action(
                discard_btn,
                handler as Id,
                sel!(onDiscardMessage:),
            );
        }
        let _: () = msg_send![bar, addSubview: discard_btn];

        // Commit button (rightmost)
        let commit_x = right_edge - btn_width;
        let commit_btn = crate::ui_helpers::create_button(
            core_graphics::geometry::CGRect::new(
                &CGPoint::new(commit_x, 3.0),
                &core_graphics::geometry::CGSize::new(btn_width, btn_height),
            ),
            "Commit",
            crate::ui_helpers::button_style::ROUNDED,
        );
        if let Some(handler) = action_handler {
            crate::ui_helpers::button_set_action(commit_btn, handler as Id, sel!(onCommitMessage:));
        }
        let _: () = msg_send![bar, addSubview: commit_btn];

        bar
    }
}

fn hide_voice_chat_overlay_impl() {
    // IMPORTANT: do not hold OVERLAY_STATE while calling `window_close`.
    // `window_close` triggers AppKit notifications/delegate callbacks (windowWillClose),
    // and those callbacks also lock OVERLAY_STATE. Holding the lock here can deadlock
    // the main thread (observed as a hard freeze/hang).
    let window_ptr = {
        let mut state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
        let window_ptr = state.window.take();
        clear_overlay_state(&mut state);
        window_ptr
    };

    if let Some(window_ptr) = window_ptr {
        unsafe {
            let window = window_ptr as Id;
            crate::ui_helpers::animate_fade(window, 0.0, 0.15);
            crate::ui_helpers::window_close(window);
        }
    }

    clear_search_field();
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
    state.active_tab = Tab::Drawer;
    state.is_sending = false;
    state.manual_draft.clear();
    state.conversation_state = ConversationModeState::Inactive;
}

fn refresh_drawer_impl() {
    let mut state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
    state.favorites = load_favorites_from_disk();
    state.drawer_entries = load_drawer_entries();
    let query = state
        .search_field
        .map(|field| unsafe { get_text_field_string(field as Id) })
        .unwrap_or_default();
    render_drawer_entries(&mut state, &query);
}

pub fn handle_card_copy(index: usize) {
    let state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(entry) = state.drawer_entries.get(index)
        && let Ok(contents) = std::fs::read_to_string(&entry.path)
    {
        copy_to_clipboard(&contents);
    }
}

pub fn handle_card_edit(index: usize) {
    let state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(entry) = state.drawer_entries.get(index) {
        let _ = open_file_in_editor(&entry.path);
    }
}

pub fn handle_card_delete(index: usize) {
    let mut state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(entry) = state.drawer_entries.get(index)
        && let Err(err) = std::fs::remove_file(&entry.path)
    {
        warn!("Failed to delete {}: {}", entry.path.display(), err);
    }
    state.favorites = load_favorites_from_disk();
    state.drawer_entries = load_drawer_entries();
    render_drawer_entries(&mut state, "");
}

pub fn handle_card_favorite(index: usize) {
    let mut state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(entry) = state.drawer_entries.get_mut(index) {
        entry.is_favorite = !entry.is_favorite;
        let key = entry.path.to_string_lossy().to_string();
        if entry.is_favorite {
            state.favorites.insert(key);
        } else {
            state.favorites.remove(&key);
        }
        save_favorites_to_disk(&state.favorites);
    }
    update_favorites_button_with_state(&mut state);
    render_drawer_entries(&mut state, "");
}

pub(super) fn toggle_drawer_favorites_only_impl() {
    let mut state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
    state.drawer_favorites_only = !state.drawer_favorites_only;

    // Jump to Drawer (this feature is Drawer-scoped).
    update_active_tab_locked(&mut state, Tab::Drawer);

    update_favorites_button_with_state(&mut state);

    let query = state
        .search_field
        .map(|field| unsafe { get_text_field_string(field as Id) })
        .unwrap_or_default();
    render_drawer_entries(&mut state, &query);
}

fn update_favorites_button_with_state(state: &mut VoiceChatOverlayState) {
    unsafe {
        let Some(btn_ptr) = state.favorites_button else {
            return;
        };
        let btn = btn_ptr as Id;
        let title = if state.drawer_favorites_only {
            "♥"
        } else {
            "♡"
        };
        let title = ns_string(title);
        let _: () = msg_send![btn, setTitle: title];
    }
}

fn render_drawer_entries(state: &mut VoiceChatOverlayState, query: &str) {
    unsafe {
        let Some(container_ptr) = state.drawer_container else {
            return;
        };
        let container = container_ptr as Id;
        stack_view_clear(container);

        let filter = query.trim().to_lowercase();
        for (index, entry) in state.drawer_entries.iter().enumerate() {
            if state.drawer_favorites_only && !entry.is_favorite {
                continue;
            }
            if !filter.is_empty() {
                let hay = entry.preview.to_lowercase();
                if !hay.contains(&filter) {
                    continue;
                }
            }
            let card = create_drawer_card(entry, index, state.action_handler);
            stack_view_add(container, card);
        }

        // Keep the scroll document height in sync with its arranged subviews; otherwise the
        // scroll view can end up showing an empty area (looks like the drawer "does nothing").
        if let Some(scroll_view_ptr) = state.drawer_scroll_view {
            let fitting: CGSize = msg_send![container, fittingSize];
            let frame: CGRect = msg_send![container, frame];
            let new_size = CGSize::new(frame.size.width, fitting.height.max(frame.size.height));
            let _: () = msg_send![container, setFrameSize: new_size];

            // Scroll to top on refresh/filter.
            let scroll_view = scroll_view_ptr as Id;
            let content_view: Id = msg_send![scroll_view, contentView];
            let _: () = msg_send![content_view, scrollToPoint: CGPoint::new(0.0, 0.0)];
            let _: () = msg_send![scroll_view, reflectScrolledClipView: content_view];
        }
    }
}

fn create_drawer_card(entry: &DrawerEntry, index: usize, handler: Option<usize>) -> Id {
    unsafe {
        let ns_view = Class::get("NSView").unwrap();
        let frame = core_graphics::geometry::CGRect::new(
            &CGPoint::new(0.0, 0.0),
            &core_graphics::geometry::CGSize::new(410.0, 120.0),
        );
        let title = format!(
            "{}  {}",
            entry_type_label(entry),
            format_relative_time(entry.timestamp)
        );
        let subtitle = format!("{} • {}", mode_label(entry.mode), entry.path.display());
        let preview = entry.preview.clone();
        let card = create_card_view(frame, &title, &subtitle, &preview);

        let actions_container: Id = msg_send![ns_view, alloc];
        let actions_frame = core_graphics::geometry::CGRect::new(
            &CGPoint::new(12.0, 8.0),
            &core_graphics::geometry::CGSize::new(386.0, 20.0),
        );
        let actions_container: Id = msg_send![actions_container, initWithFrame: actions_frame];

        let button_titles = ["Copy", "Edit", "Delete"];
        let button_actions = [sel!(onCardCopy:), sel!(onCardEdit:), sel!(onCardDelete:)];
        for (idx, title) in button_titles.iter().enumerate() {
            let button = crate::ui_helpers::create_button(
                core_graphics::geometry::CGRect::new(
                    &CGPoint::new((idx as f64) * 70.0, 0.0),
                    &core_graphics::geometry::CGSize::new(64.0, 18.0),
                ),
                title,
                crate::ui_helpers::button_style::SMALL_SQUARE,
            );
            if let Some(handler) = handler {
                crate::ui_helpers::button_set_action(button, handler as Id, button_actions[idx]);
            }
            let _: () = msg_send![button, setTag: index as isize];
            let _: () = msg_send![actions_container, addSubview: button];
        }

        let favorite = crate::ui_helpers::create_button(
            core_graphics::geometry::CGRect::new(
                &CGPoint::new(230.0, 0.0),
                &core_graphics::geometry::CGSize::new(28.0, 18.0),
            ),
            if entry.is_favorite { "♥" } else { "♡" },
            crate::ui_helpers::button_style::SMALL_SQUARE,
        );
        if let Some(handler) = handler {
            crate::ui_helpers::button_set_action(favorite, handler as Id, sel!(onCardFavorite:));
        }
        let _: () = msg_send![favorite, setTag: index as isize];
        let _: () = msg_send![actions_container, addSubview: favorite];

        let _: () = msg_send![card, addSubview: actions_container];
        card
    }
}

fn entry_type_label(entry: &DrawerEntry) -> &'static str {
    if entry.is_ai_formatted { "AI" } else { "Tt" }
}

fn mode_label(mode: TranscriptionMode) -> &'static str {
    match mode {
        TranscriptionMode::Hold => "Ctrl+Hold",
        TranscriptionMode::Assistive => "Shift/Cmd",
        TranscriptionMode::Toggle => "Toggle",
        TranscriptionMode::Conversation => "Moshi",
    }
}

fn format_relative_time(timestamp: SystemTime) -> String {
    let now = SystemTime::now();
    if let Ok(duration) = now.duration_since(timestamp) {
        let minutes = duration.as_secs() / 60;
        if minutes < 60 {
            return format!("{} min", minutes.max(1));
        }
        let hours = minutes / 60;
        if hours < 24 {
            return format!("{} h", hours);
        }
        let days = hours / 24;
        return format!("{} d", days);
    }
    "just now".to_string()
}

pub fn load_drawer_entries() -> Vec<DrawerEntry> {
    let config_dir = codescribe_core::config::Config::config_dir();
    let today = chrono::Local::now().format("%Y-%m-%d").to_string();
    let dir = config_dir.join("transcriptions").join(today);

    let favorites = load_favorites_from_disk();
    let files = list_draft_files(&dir);
    files
        .into_iter()
        .map(|path| {
            let name = path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("")
                .to_lowercase();
            let metadata = path.metadata().ok();
            let timestamp = metadata
                .as_ref()
                .and_then(|m| m.modified().ok())
                .unwrap_or_else(SystemTime::now);
            let content = std::fs::read_to_string(&path).unwrap_or_default();
            let preview = content.chars().take(120).collect::<String>();
            let mode = if name.contains("conversation") || name.contains("moshi") {
                TranscriptionMode::Conversation
            } else if name.contains("assistive") {
                TranscriptionMode::Assistive
            } else if name.contains("_raw") || name.contains("raw") {
                TranscriptionMode::Hold
            } else {
                TranscriptionMode::Toggle
            };
            let is_ai_formatted = name.contains("_ai") && !name.contains("ai-failed");
            let is_favorite = favorites.contains(&path.to_string_lossy().to_string());
            DrawerEntry {
                path,
                timestamp,
                mode,
                preview,
                is_ai_formatted,
                is_favorite,
            }
        })
        .collect()
}

#[derive(serde::Serialize, serde::Deserialize, Default)]
struct FavoritesFile {
    version: u32,
    paths: Vec<String>,
}

fn favorites_path() -> std::path::PathBuf {
    let dir = codescribe_core::config::Config::config_dir();
    dir.join("voice_chat_favorites.json")
}

fn load_favorites_from_disk() -> std::collections::HashSet<String> {
    use std::collections::HashSet;
    let path = favorites_path();
    let Ok(data) = std::fs::read_to_string(&path) else {
        return HashSet::new();
    };
    let Ok(file) = serde_json::from_str::<FavoritesFile>(&data) else {
        return HashSet::new();
    };
    file.paths.into_iter().collect()
}

fn save_favorites_to_disk(favorites: &std::collections::HashSet<String>) {
    let path = favorites_path();
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let file = FavoritesFile {
        version: 1,
        paths: favorites.iter().cloned().collect(),
    };
    if let Ok(json) = serde_json::to_string_pretty(&file) {
        let _ = std::fs::write(&path, json);
    }
}

pub fn update_drawer_after_save(path: &std::path::Path) {
    info!("Drawer entry saved: {}", path.display());
    refresh_drawer();
}
