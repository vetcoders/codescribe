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
    get_text_view_string, list_draft_files, ns_string, open_file_in_editor,
    resize_bubble_container_for_text, set_text_field_string, set_text_view_string, stack_view_add,
    stack_view_clear, update_bubble_text,
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
        state.push_message(ChatMessage {
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
        state.push_message(ChatMessage {
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
    info!("hide_voice_chat_overlay requested");
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

/// Store the name of the app the user was in when starting assistive mode.
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
    unsafe {
        let mut state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
        state.active_tab = tab;

        if let Some(tab_control) = state.tab_control {
            let _: () = msg_send![tab_control as Id, setSelectedSegment: if tab == Tab::Drawer { 0_isize } else { 1_isize }];
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
        if let Some(agent_input_bar) = state.agent_input_bar {
            crate::ui_helpers::set_hidden(agent_input_bar as Id, show_drawer);
        }
        if let Some(agent_attach) = state.agent_attach_button {
            crate::ui_helpers::set_hidden(agent_attach as Id, show_drawer);
        }
        if let Some(agent_send) = state.agent_send_button {
            crate::ui_helpers::set_hidden(agent_send as Id, show_drawer);
        }

        // When switching to Agent, make sure the input field can actually receive text.
        // We do NOT force activation (to avoid stealing focus), but if the window is already
        // key, we nudge first responder to the input field for better UX.
        if tab == Tab::Agent
            && let (Some(window_ptr), Some(input_ptr)) = (state.window, state.agent_input_text_view)
        {
            let window = window_ptr as Id;
            let is_key: bool = msg_send![window, isKeyWindow];
            if is_key {
                let _: bool = msg_send![window, makeFirstResponder: input_ptr as Id];
            }
        }
    }
}

fn update_voice_chat_status_impl(status: &str) {
    unsafe {
        let state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(title_label) = state.title_label {
            let title = format!("CodeScribe — {}", status);
            let ns_str = ns_string(&title);
            let _: () = msg_send![title_label as Id, setStringValue: ns_str];
        }
    }
}

fn append_voice_chat_user_delta_impl(delta: &str) {
    let mut state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
    ensure_streaming_user_message(&mut state);
    if let Some(last) = state.messages.last_mut() {
        codescribe_core::contracts::TranscriptDelta::from_raw(delta).apply(&mut last.text);
        last.is_streaming = true;
    }
    update_chat_view_with_state(&mut state, false);
}

fn append_voice_chat_assistant_delta_impl(delta: &str) {
    let mut state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
    ensure_streaming_assistant_message(&mut state);
    if let Some(last) = state.messages.last_mut() {
        codescribe_core::contracts::TranscriptDelta::from_raw(delta).apply(&mut last.text);
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
        let Some((bubble_ptr, label_ptr)) = state.agent_bubble_views.last().copied() else {
            return false;
        };

        let container = bubble_ptr as Id;
        let label = label_ptr as Id;
        update_bubble_text(label, &last_message.text, last_message.is_streaming);
        let display_text = display_text_for_message(last_message);
        resize_bubble_container_for_text(container, label, &display_text);

        // Keep the latest message in view while streaming.
        if let Some(scroll_view_ptr) = state.agent_scroll_view {
            let _ = scroll_view_ptr;
            let bounds: CGRect = msg_send![container, bounds];
            let _: () = msg_send![container, scrollRectToVisible: bounds];
        }
        true
    }
}

fn finalize_user_message_impl(text: &str) {
    let mut state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
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

pub(super) fn clear_voice_chat_text_impl() {
    let mut state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
    state.messages.clear();
    state.manual_draft.clear();
    state.is_sending = false;
    state.attached_files.clear();
    state.attached_files_last_sent = None;
    reset_attach_button_ui_locked(&mut state);

    if let Some(input_view) = state.agent_input_text_view {
        unsafe { set_text_view_string(input_view as Id, "") };
    } else if let Some(input_field) = state.agent_input_field {
        unsafe { set_text_field_string(input_field as Id, "") };
    }

    update_chat_view_with_state(&mut state, true);
    update_send_button_with_state(&mut state);
}

/// Send the draft message (called from handlers)
///
/// SAFETY: OVERLAY_STATE must be fully released before invoking the send
/// callback, which may re-acquire the lock from another thread/queue.
pub fn send_draft_message_impl() {
    let callback = {
        let mut state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
        let draft = if let Some(text_view) = state.agent_input_text_view {
            unsafe { get_text_view_string(text_view as Id) }
        } else if let Some(input_field) = state.agent_input_field {
            unsafe { get_text_field_string(input_field as Id) }
        } else {
            return;
        };
        let draft = draft.trim().to_string();
        if draft.is_empty() {
            return;
        }

        let attachments_to_send = attachment_should_include_locked(&state);
        if let Some((fingerprint, _paths, summary)) = attachments_to_send.as_ref() {
            state.push_message(ChatMessage {
                role: ChatRole::System,
                text: format!("📎 Załączniki (wysłane raz): {}", summary),
                is_streaming: false,
                is_error: false,
            });
            state.attached_files_last_sent = Some(*fingerprint);
        }

        state.push_message(ChatMessage {
            role: ChatRole::User,
            text: draft.clone(),
            is_streaming: false,
            is_error: false,
        });
        state.manual_draft.clear();
        state.is_sending = true;
        if let Some(text_view) = state.agent_input_text_view {
            unsafe { set_text_view_string(text_view as Id, "") };
        } else if let Some(input_field) = state.agent_input_field {
            unsafe { set_text_field_string(input_field as Id, "") };
        }
        update_chat_view_with_state(&mut state, true);
        update_send_button_with_state(&mut state);

        let handler = SEND_CALLBACK.lock().unwrap_or_else(|e| e.into_inner());
        (handler.clone(), draft, attachments_to_send)
    };

    if let (Some(handler), draft, attachments_to_send) = callback {
        if let Some((_fingerprint, paths, _summary)) = attachments_to_send {
            std::thread::spawn(move || {
                let block = build_attachments_block(&paths);
                let payload = if block.is_empty() {
                    draft
                } else {
                    format!("{draft}\n\n{block}")
                };
                // The send callback uses `tokio::spawn`, which requires a runtime handle.
                // Calling it from an arbitrary background thread can panic (release builds abort).
                Queue::main().exec_async(move || handler(payload));
            });
        } else {
            handler(draft);
        }
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
        let attachments_to_send = attachment_should_include_locked(&state);
        if let Some((fingerprint, _paths, summary)) = attachments_to_send.as_ref() {
            state.push_message(ChatMessage {
                role: ChatRole::System,
                text: format!("📎 Załączniki (wysłane raz): {}", summary),
                is_streaming: false,
                is_error: false,
            });
            state.attached_files_last_sent = Some(*fingerprint);
        }
        state.is_sending = true;
        update_chat_view_with_state(&mut state, true);
        update_send_button_with_state(&mut state);
        let handler = SEND_CALLBACK.lock().unwrap_or_else(|e| e.into_inner());
        (handler.clone(), text, attachments_to_send)
    };

    if let (Some(handler), text, attachments_to_send) = callback {
        if let Some((_fingerprint, paths, _summary)) = attachments_to_send {
            std::thread::spawn(move || {
                let block = build_attachments_block(&paths);
                let payload = if block.is_empty() {
                    text
                } else {
                    format!("{text}\n\n{block}")
                };
                Queue::main().exec_async(move || handler(payload));
            });
        } else {
            handler(text);
        }
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
        state.push_message(ChatMessage {
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
        state.push_message(ChatMessage {
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

        if scroll_to_bottom && let Some(scroll_view_ptr) = state.agent_scroll_view {
            let scroll_view = scroll_view_ptr as Id;
            let content_view: Id = msg_send![scroll_view, contentView];
            let _: () = msg_send![content_view, scrollToPoint: CGPoint::new(0.0, f64::MAX)];
        }
    }
}

fn update_send_button_with_state(state: &mut VoiceChatOverlayState) {
    unsafe {
        if let Some(button_ptr) = state.agent_send_button {
            let btn = button_ptr as Id;
            // In auto-send mode: button shows "Auto" and is disabled (voice input sends
            // automatically). In draft mode: button shows ">" and the user clicks to send.
            let enabled = !state.is_sending;
            let _: () = msg_send![btn, setEnabled: enabled];
            let title = if state.is_sending {
                "…"
            } else if state.auto_send_enabled {
                "Auto"
            } else {
                ">"
            };
            let label = if state.auto_send_enabled {
                "Auto-send enabled"
            } else {
                "Send message"
            };
            let _: () = msg_send![btn, setTitle: ns_string(title)];
            let _: () = msg_send![btn, setAccessibilityLabel: ns_string(label)];
        }
    }
}

fn reset_attach_button_ui_locked(state: &mut VoiceChatOverlayState) {
    unsafe {
        let Some(btn_ptr) = state.agent_attach_button else {
            return;
        };
        let btn = btn_ptr as Id;
        let _: () = msg_send![btn, setTitle: ns_string("📎")];
        crate::ui_helpers::set_tooltip(btn, "Załącz pliki (kontekst dla asystenta)");
    }
}

fn attachment_should_include_locked(
    state: &VoiceChatOverlayState,
) -> Option<(u64, Vec<std::path::PathBuf>, String)> {
    if state.attached_files.is_empty() {
        return None;
    }
    let fingerprint = attachment_fingerprint(&state.attached_files);
    if state.attached_files_last_sent == Some(fingerprint) {
        return None;
    }
    let summary = attachment_summary(&state.attached_files);
    Some((fingerprint, state.attached_files.clone(), summary))
}

fn attachment_summary(paths: &[std::path::PathBuf]) -> String {
    let mut names: Vec<String> = paths
        .iter()
        .filter_map(|p| p.file_name().map(|n| n.to_string_lossy().to_string()))
        .collect();
    names.sort();
    if names.len() <= 3 {
        names.join(", ")
    } else {
        format!("{}, … (+{})", names[..3].join(", "), names.len() - 3)
    }
}

fn attachment_fingerprint(paths: &[std::path::PathBuf]) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    for p in paths {
        p.hash(&mut hasher);
        if let Ok(meta) = std::fs::metadata(p) {
            meta.len().hash(&mut hasher);
            meta.modified().ok().hash(&mut hasher);
        }
    }
    hasher.finish()
}

fn build_attachments_block(paths: &[std::path::PathBuf]) -> String {
    use std::io::Read;

    const MAX_TOTAL_CHARS: usize = 120_000;
    const MAX_FILE_CHARS: usize = 40_000;
    const MAX_FILE_BYTES: usize = 512 * 1024; // cap IO; we only inline a prefix anyway

    let mut out = String::new();
    out.push_str("ATTACHMENTS (file context)\n");

    let mut total_chars = out.chars().count();
    for path in paths {
        if total_chars >= MAX_TOTAL_CHARS {
            break;
        }

        let display = path.to_string_lossy();
        out.push_str("\n---\n");
        out.push_str(&format!("FILE: {display}\n"));

        let Ok(mut f) = std::fs::File::open(path) else {
            out.push_str("(failed to open)\n");
            continue;
        };

        let mut buf = Vec::new();
        let _ = (&mut f)
            .take(MAX_FILE_BYTES as u64)
            .read_to_end(&mut buf);

        let Ok(mut s) = String::from_utf8(buf) else {
            out.push_str("(skipped: not UTF-8 text)\n");
            continue;
        };

        // Normalize + cap per-file.
        if s.chars().count() > MAX_FILE_CHARS {
            s = s.chars().take(MAX_FILE_CHARS).collect();
            s.push_str("\n… (truncated)\n");
        }

        // Cap total.
        let remaining = MAX_TOTAL_CHARS.saturating_sub(total_chars);
        if remaining == 0 {
            break;
        }
        let mut snippet: String = s.chars().take(remaining).collect();
        if snippet.len() < s.len() {
            snippet.push_str("\n… (truncated)\n");
        }

        out.push_str("```text\n");
        out.push_str(&snippet);
        if !snippet.ends_with('\n') {
            out.push('\n');
        }
        out.push_str("```\n");

        total_chars = out.chars().count();
    }

    out
}

/// Resize the Agent input bar based on current draft text.
///
/// Keeps it compact by default, and grows it when the user types/pastes longer messages.
pub fn resize_agent_input_to_draft() {
    let mut state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
    resize_agent_input_locked(&mut state);
}

fn resize_agent_input_locked(state: &mut VoiceChatOverlayState) {
    unsafe {
        let (
            Some(window_ptr),
            Some(bar_ptr),
            Some(scroll_ptr),
            Some(text_view_ptr),
            Some(attach_ptr),
            Some(send_ptr),
        ) = (
            state.window,
            state.agent_input_bar,
            state.agent_input_scroll_view,
            state.agent_input_text_view,
            state.agent_attach_button,
            state.agent_send_button,
        )
        else {
            return;
        };

        let window = window_ptr as Id;
        let input_bar = bar_ptr as Id;
        let input_scroll = scroll_ptr as Id;
        let text_view = text_view_ptr as Id;
        let attach_btn = attach_ptr as Id;
        let send_btn = send_ptr as Id;

        let window_frame: CGRect = msg_send![window, frame];
        let window_width = window_frame.size.width;
        let window_height = window_frame.size.height;

        let text = get_text_view_string(text_view);
        let hard_lines = (text.matches('\n').count() + 1).max(1);
        // Heuristic for wrapped lines: assume ~52 chars per visual line at this width.
        let wrapped_lines = text.chars().count().div_ceil(52).max(1);
        let visual_lines = hard_lines.max(wrapped_lines);

        // Keep the input compact by default (single-line-ish), then grow smoothly up to a cap.
        let min_h = 44.0;
        let max_h = 140.0;
        let line_h = 18.0;
        let desired_h = if text.trim().is_empty() {
            min_h
        } else {
            (min_h + (visual_lines.saturating_sub(1) as f64) * line_h).clamp(min_h, max_h)
        };

        let current_bar: CGRect = msg_send![input_bar, frame];
        if (current_bar.size.height - desired_h).abs() < 0.5 {
            return;
        }

        // Resize input bar (anchored to bottom).
        let new_bar_frame = CGRect::new(
            &CGPoint::new(current_bar.origin.x, current_bar.origin.y),
            &CGSize::new(window_width - 32.0, desired_h),
        );
        let _: () = msg_send![input_bar, setFrame: new_bar_frame];

        // Resize the scrollable text view inside the bar.
        let text_area_frame = CGRect::new(
            &CGPoint::new(12.0, 10.0),
            &CGSize::new(window_width - 140.0, (desired_h - 20.0).max(24.0)),
        );
        let _: () = msg_send![input_scroll, setFrame: text_area_frame];

        // Recenter buttons vertically.
        let send_y = ((desired_h - 32.0) / 2.0).max(8.0);
        let attach_frame = CGRect::new(
            &CGPoint::new(window_width - 120.0, send_y),
            &CGSize::new(36.0, 32.0),
        );
        let _: () = msg_send![attach_btn, setFrame: attach_frame];
        let send_frame = CGRect::new(
            &CGPoint::new(window_width - 76.0, send_y),
            &CGSize::new(36.0, 32.0),
        );
        let _: () = msg_send![send_btn, setFrame: send_frame];

        // Resize Agent scroll view so it doesn't overlap with input.
        if let Some(agent_scroll_ptr) = state.agent_scroll_view {
            let agent_scroll = agent_scroll_ptr as Id;
            let header_height = 44.0;
            let bottom = desired_h + 18.0;
            let top = window_height - header_height - 10.0;
            let new_agent_frame = CGRect::new(
                &CGPoint::new(16.0, bottom),
                &CGSize::new(window_width - 32.0, (top - bottom).max(0.0)),
            );
            let _: () = msg_send![agent_scroll, setFrame: new_agent_frame];

            if let Some(container_ptr) = state.agent_container {
                let container = container_ptr as Id;
                let container_frame: CGRect = msg_send![container, frame];
                // IMPORTANT: do NOT clamp the document view height to the visible clip height.
                // That disables scrolling and makes long agent replies unscrollable.
                let new_size = CGSize::new(new_agent_frame.size.width, container_frame.size.height);
                let _: () = msg_send![container, setFrameSize: new_size];
            }
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
    unsafe {
        let mut state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(window_ptr) = state.window {
            let window = window_ptr as Id;
            info!("Voice chat overlay hide: closing window");
            crate::ui_helpers::animate_fade(window, 0.0, 0.15);
            crate::ui_helpers::window_close(window);
            clear_overlay_state(&mut state);
        } else {
            debug!("Voice chat overlay hide: no window to close");
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
    state.agent_input_bar = None;
    state.agent_input_scroll_view = None;
    state.agent_input_text_view = None;
    state.agent_input_field = None;
    state.agent_attach_button = None;
    state.agent_send_button = None;
    state.attached_files.clear();
    state.attached_files_last_sent = None;
    state.active_tab = Tab::Drawer;
    state.is_sending = false;
    state.manual_draft.clear();
    state.conversation_state = ConversationModeState::Inactive;
}

fn refresh_drawer_impl() {
    let mut state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
    state.drawer_entries = load_drawer_entries();
    let query = state
        .search_field
        .map(|field| unsafe { get_text_field_string(field as Id) })
        .unwrap_or_default();
    render_drawer_entries(&mut state, &query);
}

/// Check that a path is within the CodeScribe transcriptions directory.
/// Prevents accidental read/delete of files outside the sandbox.
fn is_safe_transcription_path(path: &std::path::Path) -> bool {
    let config_dir = codescribe_core::config::Config::config_dir();
    let allowed_root = config_dir.join("transcriptions");
    match (path.canonicalize(), allowed_root.canonicalize()) {
        (Ok(canon), Ok(root)) => canon.starts_with(root),
        // If canonicalize fails (file doesn't exist yet), fall back to prefix check
        _ => path.starts_with(&allowed_root),
    }
}

pub fn handle_card_copy(index: usize) {
    let state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(entry) = state.drawer_entries.get(index) {
        if !is_safe_transcription_path(&entry.path) {
            warn!(
                "Blocked copy: path outside transcriptions dir: {}",
                entry.path.display()
            );
            return;
        }
        if let Ok(contents) = std::fs::read_to_string(&entry.path) {
            copy_to_clipboard(&contents);
        }
    }
}

pub fn handle_card_edit(index: usize) {
    let state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(entry) = state.drawer_entries.get(index) {
        if !is_safe_transcription_path(&entry.path) {
            warn!(
                "Blocked edit: path outside transcriptions dir: {}",
                entry.path.display()
            );
            return;
        }
        let _ = open_file_in_editor(&entry.path);
    }
}

pub fn handle_card_delete(index: usize) {
    let mut state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(entry) = state.drawer_entries.get(index) {
        if !is_safe_transcription_path(&entry.path) {
            warn!(
                "Blocked delete: path outside transcriptions dir: {}",
                entry.path.display()
            );
            return;
        }
        if let Err(err) = std::fs::remove_file(&entry.path) {
            warn!("Failed to delete {}: {}", entry.path.display(), err);
        }
    }
    state.drawer_entries = load_drawer_entries();
    render_drawer_entries(&mut state, "");
}

pub fn handle_card_favorite(index: usize) {
    let mut state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(entry) = state.drawer_entries.get_mut(index) {
        entry.is_favorite = !entry.is_favorite;
    }
    render_drawer_entries(&mut state, "");
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
            if !filter.is_empty() {
                let hay = entry.preview.to_lowercase();
                if !hay.contains(&filter) {
                    continue;
                }
            }
            let card = create_drawer_card(entry, index, state.action_handler);
            stack_view_add(container, card);
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
        TranscriptionMode::Assistive => "Ctrl+Shift",
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
            DrawerEntry {
                path,
                timestamp,
                mode,
                preview,
                is_ai_formatted,
                is_favorite: false,
            }
        })
        .collect()
}

pub fn update_drawer_after_save(path: &std::path::Path) {
    info!("Drawer entry saved: {}", path.display());
    refresh_drawer();
}
