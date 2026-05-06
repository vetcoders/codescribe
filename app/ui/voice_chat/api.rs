//! Public API and internal helpers for voice chat overlay
//!
//! Contains all the public functions for controlling the overlay and
//! internal helper functions for state updates.

use core_graphics::geometry::{CGPoint, CGRect, CGSize};
use dispatch::Queue;
use objc::runtime::{Class, Object};
use objc::{msg_send, sel, sel_impl};
use std::collections::HashSet;
use std::path::PathBuf;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tracing::{debug, info, warn};

use chrono::{DateTime, Local};

use codescribe_core::agent::{Thread, ThreadIndex, ThreadStore};
use codescribe_core::attachment::Attachment;

use crate::ui::shared::status::{UiStatus, status_from_detail};
use crate::ui_helpers::{
    BubbleConfig, BubbleRole, LabelConfig, add_subview, apply_tafla_surface, button_set_action,
    button_style, chat_header_layout, color_label, color_rgba, color_secondary_label,
    create_bubble_view, create_button, create_card_view, create_label, get_text_field_string,
    get_text_view_string, layout_region_frame_for_view, ns_string, open_file_in_editor,
    resize_bubble_container_for_text, set_button_symbol, set_text_field_string,
    set_text_view_string, set_tooltip, stack_view_add, stack_view_clear, ui_colors, ui_tokens,
    update_bubble_text, window_set_alpha, window_show,
};

use super::handlers::{clear_search_field, copy_to_clipboard};
use super::state::{
    ChatMessage, ChatRole, ConversationModeState, DrawerEntry, DrawerEntrySource, OVERLAY_STATE,
    SEND_CALLBACK, Tab, TranscriptionMode, VoiceChatOverlayState,
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

/// Persist runtime degradation state used by status and tooltip rendering.
pub fn set_voice_chat_runtime_degraded(is_degraded: bool, reason: Option<&str>) {
    let reason_owned = reason.map(str::to_string);
    Queue::main().exec_async(move || {
        set_voice_chat_runtime_degraded_impl(is_degraded, reason_owned);
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

/// Finalize the latest user message without changing its text.
pub fn finalize_voice_chat_user_message() {
    Queue::main().exec_async(|| {
        finalize_user_message_state_only_impl();
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

/// Finalize the latest assistant message without changing its text.
pub fn finalize_voice_chat_assistant_message() {
    Queue::main().exec_async(|| {
        finalize_assistant_message_state_only_impl(false);
    });
}

/// Add an error message to the chat log
pub fn add_voice_chat_error_message(text: &str) {
    let text_owned = text.to_string();
    Queue::main().exec_async(move || {
        let mut state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
        state.active_assistant_stream_index = None;
        let mode = message_mode_label(&state);
        state.messages.push(ChatMessage {
            role: ChatRole::System,
            text: text_owned.clone(),
            is_streaming: false,
            is_error: true,
            timestamp: SystemTime::now(),
            mode: Some(mode),
        });
        state.is_sending = false;
        update_chat_view_with_state(&mut state, true);
        update_send_button_with_state(&mut state);
    });
}

/// Add a non-error system message to the chat log.
pub fn add_voice_chat_system_message(text: &str) {
    let text_owned = text.to_string();
    Queue::main().exec_async(move || {
        let mut state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
        let mode = message_mode_label(&state);
        state.messages.push(ChatMessage {
            role: ChatRole::System,
            text: text_owned.clone(),
            is_streaming: false,
            is_error: false,
            timestamp: SystemTime::now(),
            mode: Some(mode),
        });
        update_chat_view_with_state(&mut state, true);
    });
}

/// Add a user message to the chat
pub fn add_voice_chat_user_message(text: &str) {
    let text_owned = text.to_string();
    Queue::main().exec_async(move || {
        let mut state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
        state.active_user_stream_index = None;
        let mode = message_mode_label(&state);
        state.messages.push(ChatMessage {
            role: ChatRole::User,
            text: text_owned,
            is_streaming: false,
            is_error: false,
            timestamp: SystemTime::now(),
            mode: Some(mode),
        });
        update_chat_view_with_state(&mut state, true);
    });
}

/// Seed chat with a transcript and submit it as a user message.
///
/// This is used by transcription-overlay `Augment` to perform an explicit handoff
/// from dictation to chat.
pub fn handoff_transcript_to_chat(transcript: &str) {
    let transcript_owned = transcript.trim().to_string();
    if transcript_owned.is_empty() {
        return;
    }

    Queue::main().exec_async(move || {
        handoff_transcript_to_chat_impl(&transcript_owned);
    });
}

/// Dispatch a payload through the registered chat send callback without mutating bubbles.
///
/// Returns `true` when a callback was found and invoked.
pub fn dispatch_voice_chat_send(payload: &str) -> bool {
    let payload = payload.trim();
    if payload.is_empty() {
        return false;
    }
    let handler = {
        let guard = SEND_CALLBACK.lock().unwrap_or_else(|e| e.into_inner());
        guard.clone()
    };
    if let Some(handler) = handler {
        handler(payload.to_string());
        true
    } else {
        warn!("No voice-chat send callback set; cannot dispatch runtime send request");
        false
    }
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

/// Start a fresh Agent thread by rotating backend runtime first, then clearing UI state.
pub(super) fn start_new_thread_impl() {
    update_voice_chat_status_impl("Starting new thread...");

    std::thread::spawn(|| {
        let rt = match tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
        {
            Ok(rt) => rt,
            Err(error) => {
                let reason = format!("Unable to initialize async runtime for New thread: {error}");
                Queue::main().exec_async(move || {
                    warn!("{reason}");
                    update_voice_chat_status_impl("Thread reset failed");
                    add_voice_chat_error_message(&reason);
                });
                return;
            }
        };

        let reset_result = rt.block_on(crate::controller::reset_agent_runtime_for_new_thread());
        Queue::main().exec_async(move || match reset_result {
            Ok(generation) => {
                clear_voice_chat_text_impl();
                update_voice_chat_status_impl("Ready");
                info!("New thread started (generation={generation})");
            }
            Err(error) => {
                warn!("Failed to start new thread: {error}");
                update_voice_chat_status_impl("Thread reset failed");
                add_voice_chat_error_message(&format!(
                    "Unable to start a new thread. Continuing the current thread. {error}"
                ));
            }
        });
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

/// Export the current Agent chat thread as Markdown.
///
/// - `assistant_only=false` → include User + Assistant messages
/// - `assistant_only=true` → include only Assistant messages
pub fn export_chat_markdown(assistant_only: bool) -> String {
    let state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
    chat_markdown_from_messages(&state.messages, assistant_only)
}

/// Save the current Agent chat thread as a `.md` file in `~/.codescribe/transcriptions/YYYY-MM-DD/`.
///
/// Returns the created path on success.
pub fn save_chat_markdown_to_history(assistant_only: bool) -> Option<PathBuf> {
    let md = export_chat_markdown(assistant_only);
    if md.trim().is_empty() {
        return None;
    }

    let now = Local::now();
    let dir = crate::state::history::transcriptions_dir(&now);
    let time_base = now.format("%H%M%S").to_string();
    let kind = if assistant_only {
        "chat-assistant"
    } else {
        "chat"
    };

    let mut candidate = dir.join(format!("{}_{}.md", time_base, kind));
    for i in 1..=10_000 {
        if !candidate.exists() {
            break;
        }
        candidate = dir.join(format!("{}_{}_{}.md", time_base, kind, i));
    }

    if std::fs::write(&candidate, md).is_ok() {
        Some(candidate)
    } else {
        None
    }
}

/// Filter drawer entries by query (reloads from disk)
pub fn filter_drawer(query: &str) {
    let query_owned = query.to_string();
    Queue::main().exec_async(move || {
        let mut state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
        state.drawer_entries = load_drawer_entries_with_query(&query_owned);
        render_drawer_entries(&mut state, &query_owned);
    });
}

/// Switch to Agent tab programmatically
pub fn show_agent_tab() {
    Queue::main().exec_async(|| {
        // If the overlay isn't created yet, defer tab selection until build completes.
        let mut state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
        if state.window.is_none() {
            state.pending_tab = Some(Tab::Agent);
            state.active_tab = Tab::Agent;
            return;
        }
        drop(state);
        update_active_tab_impl(Tab::Agent);
    });
}

/// Switch to Drawer tab programmatically
pub fn show_drawer_tab() {
    Queue::main().exec_async(|| {
        let mut state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
        if state.window.is_none() {
            state.pending_tab = Some(Tab::Drawer);
            state.active_tab = Tab::Drawer;
            return;
        }
        drop(state);
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
    // DEADLOCK PREVENTION: extract widget pointers under lock, drop lock before
    // AppKit calls (setCollapsed can animate and spin a nested run-loop).
    let (
        _prev_tab,
        tab_drawer_btn,
        tab_agent_btn,
        tab_settings_btn,
        sidebar_item,
        content_item,
        split_vc,
        drawer_sv,
        search_f,
        search_l,
        fav_btn,
        drawer_edge,
        agent_sv,
        agent_bar,
        agent_attach,
        agent_send,
        title_label,
        window_ptr,
        agent_input_tv,
        need_chat_update,
    ) = {
        let mut state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
        let prev = state.active_tab;
        state.active_tab = tab;
        (
            prev, // kept to compute need_chat_update below
            state.tab_drawer_button,
            state.tab_agent_button,
            state.tab_settings_button,
            state.split_sidebar_item,
            state.split_content_item,
            state.split_view_controller,
            state.drawer_scroll_view,
            state.search_field,
            state.search_label,
            state.favorites_button,
            state.drawer_edge_effect,
            state.agent_scroll_view,
            state.agent_input_bar,
            state.agent_attach_button,
            state.agent_send_button,
            state.title_label,
            state.window,
            state.agent_input_text_view,
            tab == Tab::Agent && prev != Tab::Agent,
        )
    }; // Lock dropped.

    let show_drawer = tab == Tab::Drawer;
    let show_agent = tab == Tab::Agent;

    unsafe {
        if let Some(b) = tab_drawer_btn {
            crate::ui_helpers::set_tab_button_active(b as Id, show_drawer);
        }
        if let Some(b) = tab_agent_btn {
            crate::ui_helpers::set_tab_button_active(b as Id, show_agent);
        }
        if let Some(b) = tab_settings_btn {
            crate::ui_helpers::set_tab_button_active(b as Id, false);
        }
        if let Some(p) = sidebar_item {
            let _: () = msg_send![p as Id, setCollapsed: show_agent];
        }
        if let Some(p) = content_item {
            let _: () = msg_send![p as Id, setCollapsed: !show_agent];
        }
        if let Some(p) = split_vc {
            let split_view: Id = msg_send![p as Id, view];
            if !split_view.is_null() {
                crate::ui_helpers::set_hidden(split_view, false);
            }
        }
        if let Some(p) = drawer_sv {
            crate::ui_helpers::set_hidden(p as Id, !show_drawer);
        }
        if let Some(p) = search_f {
            crate::ui_helpers::set_hidden(p as Id, !show_drawer);
        }
        if let Some(p) = search_l {
            crate::ui_helpers::set_hidden(p as Id, !show_drawer);
        }
        if let Some(p) = fav_btn {
            crate::ui_helpers::set_hidden(p as Id, !show_drawer);
        }
        if let Some(p) = drawer_edge {
            crate::ui_helpers::set_hidden(p as Id, !show_drawer);
        }
        if let Some(p) = agent_sv {
            crate::ui_helpers::set_hidden(p as Id, !show_agent);
        }
        if let Some(p) = agent_bar {
            crate::ui_helpers::set_hidden(p as Id, !show_agent);
        }
        if let Some(p) = agent_attach {
            crate::ui_helpers::set_hidden(p as Id, !show_agent);
        }
        if let Some(p) = agent_send {
            crate::ui_helpers::set_hidden(p as Id, !show_agent);
        }
        if let Some(p) = title_label {
            crate::ui_helpers::set_hidden(p as Id, !show_agent);
        }

        // Complex agent-tab operations need full state access; re-lock briefly.
        if show_agent {
            let mut state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
            if need_chat_update {
                update_chat_view_with_state(&mut state, true);
            }
            resize_agent_input_locked(&mut state);
        }

        // Nudge first responder to agent input when window is already key.
        if tab == Tab::Agent
            && let (Some(w), Some(inp)) = (window_ptr, agent_input_tv)
        {
            let window = w as Id;
            let is_key: bool = msg_send![window, isKeyWindow];
            if is_key {
                let _: bool = msg_send![window, makeFirstResponder: inp as Id];
            }
        }
    }
}

fn update_active_tab_locked(state: &mut VoiceChatOverlayState, tab: Tab) {
    unsafe {
        let prev_tab = state.active_tab;
        state.active_tab = tab;

        if let Some(button) = state.tab_drawer_button {
            crate::ui_helpers::set_tab_button_active(button as Id, tab == Tab::Drawer);
        }
        if let Some(button) = state.tab_agent_button {
            crate::ui_helpers::set_tab_button_active(button as Id, tab == Tab::Agent);
        }
        if let Some(button) = state.tab_settings_button {
            crate::ui_helpers::set_tab_button_active(button as Id, false);
        }

        let show_drawer = tab == Tab::Drawer;
        let show_agent = tab == Tab::Agent;

        if let Some(sidebar_item) = state.split_sidebar_item {
            let item = sidebar_item as Id;
            let _: () = msg_send![item, setCollapsed: show_agent];
        }
        if let Some(content_item) = state.split_content_item {
            let item = content_item as Id;
            let _: () = msg_send![item, setCollapsed: !show_agent];
        }
        if let Some(split_controller) = state.split_view_controller {
            let split_view: Id = msg_send![split_controller as Id, view];
            if !split_view.is_null() {
                crate::ui_helpers::set_hidden(split_view, false);
            }
        }
        if let Some(drawer_view) = state.drawer_scroll_view {
            crate::ui_helpers::set_hidden(drawer_view as Id, !show_drawer);
        }
        if let Some(search_field) = state.search_field {
            crate::ui_helpers::set_hidden(search_field as Id, !show_drawer);
        }
        if let Some(search_label) = state.search_label {
            crate::ui_helpers::set_hidden(search_label as Id, !show_drawer);
        }
        if let Some(favorites_button) = state.favorites_button {
            crate::ui_helpers::set_hidden(favorites_button as Id, !show_drawer);
        }
        if let Some(edge) = state.drawer_edge_effect {
            crate::ui_helpers::set_hidden(edge as Id, !show_drawer);
        }
        if let Some(agent_view) = state.agent_scroll_view {
            crate::ui_helpers::set_hidden(agent_view as Id, !show_agent);
        }
        if let Some(agent_input_bar) = state.agent_input_bar {
            crate::ui_helpers::set_hidden(agent_input_bar as Id, !show_agent);
        }
        if let Some(agent_attach) = state.agent_attach_button {
            crate::ui_helpers::set_hidden(agent_attach as Id, !show_agent);
        }
        if let Some(agent_send) = state.agent_send_button {
            crate::ui_helpers::set_hidden(agent_send as Id, !show_agent);
        }
        if let Some(title_label) = state.title_label {
            crate::ui_helpers::set_hidden(title_label as Id, !show_agent);
        }

        if show_agent {
            // Populate the Agent view on tab switch so the empty-state CTA is visible.
            // Important: do this only on transition to Agent; `ensure_agent_tab_visible` can
            // call `update_active_tab_locked(Tab::Agent)` frequently during streaming.
            if prev_tab != Tab::Agent {
                update_chat_view_with_state(state, true);
            }
            resize_agent_input_locked(state);
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

/// Reflow Agent layout after the overlay window was resized.
///
/// Without this, long messages can look clipped until the next message arrives.
pub(super) fn reflow_agent_after_resize_impl() {
    let Ok(mut state) = OVERLAY_STATE.try_lock() else {
        return;
    };
    if state.active_tab != Tab::Agent {
        return;
    }

    update_chat_view_with_state(&mut state, false);
    resize_agent_input_locked(&mut state);
}

/// Lightweight layout pass for window resizing (keeps inputs/footers aligned).
pub(super) fn reflow_overlay_after_resize_impl() {
    let Ok(mut state) = OVERLAY_STATE.try_lock() else {
        return;
    };
    reflow_header_controls_locked(&mut state);
    reflow_footer_controls_locked(&mut state);
    resize_agent_input_locked(&mut state);
}

fn reflow_header_controls_locked(state: &mut VoiceChatOverlayState) {
    unsafe {
        let (
            Some(drawer_ptr),
            Some(agent_ptr),
            Some(settings_ptr),
            Some(favorites_ptr),
            Some(status_ptr),
        ) = (
            state.tab_drawer_button,
            state.tab_agent_button,
            state.tab_settings_button,
            state.favorites_button,
            state.status_pill,
        )
        else {
            return;
        };

        let tab_drawer_button = drawer_ptr as Id;
        let tab_agent_button = agent_ptr as Id;
        let tab_settings_button = settings_ptr as Id;
        let favorites_button = favorites_ptr as Id;
        let status_pill = status_ptr as Id;

        let favorites_frame: CGRect = msg_send![favorites_button, frame];
        let right_cluster_start_x = favorites_frame.origin.x
            - (ui_tokens::CHAT_HEADER_BUTTON_SIZE + ui_tokens::CHAT_HEADER_BUTTON_GAP);

        let header_safe_x = ui_tokens::TRAFFIC_LIGHTS_SPACER_WIDTH + 6.0;
        let layout = chat_header_layout(header_safe_x, 0.0, right_cluster_start_x);

        let drawer_frame: CGRect = msg_send![tab_drawer_button, frame];
        let tab_y = drawer_frame.origin.y;
        let tab_h = drawer_frame.size.height.max(20.0);
        let tab_w = layout.tab_button_width.max(0.0);
        let tab_gap = layout.tab_button_gap.max(0.0);

        let tab_drawer_frame = CGRect::new(
            &CGPoint::new(layout.tab_cluster_x, tab_y),
            &CGSize::new(tab_w, tab_h),
        );
        let _: () = msg_send![tab_drawer_button, setFrame: tab_drawer_frame];

        let tab_agent_frame = CGRect::new(
            &CGPoint::new(layout.tab_cluster_x + tab_w + tab_gap, tab_y),
            &CGSize::new(tab_w, tab_h),
        );
        let _: () = msg_send![tab_agent_button, setFrame: tab_agent_frame];

        let tab_settings_frame = CGRect::new(
            &CGPoint::new(layout.tab_cluster_x + (tab_w + tab_gap) * 2.0, tab_y),
            &CGSize::new(tab_w, tab_h),
        );
        let _: () = msg_send![tab_settings_button, setFrame: tab_settings_frame];

        let status_h = ui_tokens::STATUS_PILL_HEIGHT;
        let status_y = (tab_y + (tab_h - status_h) * 0.5).max(0.0);
        let status_frame = CGRect::new(
            &CGPoint::new(layout.status_pill_x, status_y),
            &CGSize::new(layout.status_pill_width.max(0.0), status_h),
        );
        let _: () = msg_send![status_pill, setFrame: status_frame];
        let _: () = msg_send![status_pill, setHidden: !layout.show_status_pill];

        if let Some(dot_ptr) = state.status_pill_dot {
            let dot = dot_ptr as Id;
            let dot_size = ui_tokens::STATUS_DOT_SIZE;
            let dot_frame = CGRect::new(
                &CGPoint::new(
                    ui_tokens::STATUS_PILL_DOT_INSET_X,
                    (status_h - dot_size) * 0.5,
                ),
                &CGSize::new(dot_size, dot_size),
            );
            let _: () = msg_send![dot, setFrame: dot_frame];
        }

        if let Some(label_ptr) = state.status_pill_label {
            let label = label_ptr as Id;
            let label_width = (layout.status_pill_width
                - ui_tokens::STATUS_PILL_LABEL_INSET_X
                - ui_tokens::STATUS_PILL_LABEL_INSET_RIGHT)
                .max(0.0);
            let label_frame = CGRect::new(
                &CGPoint::new(ui_tokens::STATUS_PILL_LABEL_INSET_X, 1.0),
                &CGSize::new(label_width, (status_h - 2.0).max(0.0)),
            );
            let _: () = msg_send![label, setFrame: label_frame];
        }
    }
}

fn reflow_footer_controls_locked(state: &mut VoiceChatOverlayState) {
    unsafe {
        let Some(blur_ptr) = state.blur_view else {
            return;
        };
        let blur_view = blur_ptr as Id;
        let bounds: CGRect = msg_send![blur_view, bounds];
        let content_bounds = layout_region_frame_for_view(blur_view).unwrap_or(bounds);

        let footer_height = ui_tokens::FOOTER_HEIGHT;
        let footer_base_y = content_bounds.origin.y;
        let content_pad = ui_tokens::EDGE_PADDING;
        let search_x = content_bounds.origin.x + content_pad;
        let search_w = (content_bounds.size.width - content_pad * 2.0).max(160.0);

        if let Some(label_ptr) = state.search_label {
            let label = label_ptr as Id;
            let frame = CGRect::new(
                &CGPoint::new(search_x, footer_base_y + footer_height - 20.0),
                &CGSize::new(search_w, 16.0),
            );
            let _: () = msg_send![label, setFrame: frame];
        }

        if let Some(field_ptr) = state.search_field {
            let field = field_ptr as Id;
            let frame = CGRect::new(
                &CGPoint::new(search_x, footer_base_y + 12.0),
                &CGSize::new(search_w, 24.0),
            );
            let _: () = msg_send![field, setFrame: frame];
        }

        if let Some(label_ptr) = state.title_label {
            let label = label_ptr as Id;
            let label_w = ui_tokens::CHAT_TITLE_LABEL_WIDTH;
            let label_h = 16.0;
            let frame = CGRect::new(
                &CGPoint::new(
                    content_bounds.origin.x + content_bounds.size.width - content_pad - label_w,
                    footer_base_y + ((footer_height - label_h) / 2.0).max(4.0),
                ),
                &CGSize::new(label_w, label_h),
            );
            let _: () = msg_send![label, setFrame: frame];
        }

        let header_height = ui_tokens::HEADER_HEIGHT_COMPACT;
        let content_gap = ui_tokens::CONTENT_GAP;
        let content_frame = CGRect::new(
            &CGPoint::new(
                content_bounds.origin.x + content_pad,
                content_bounds.origin.y + footer_height + content_gap,
            ),
            &CGSize::new(
                (content_bounds.size.width - content_pad * 2.0).max(0.0),
                (content_bounds.size.height - header_height - footer_height - content_gap * 2.0)
                    .max(0.0),
            ),
        );

        if let Some(split_controller) = state.split_view_controller {
            let split_view: Id = msg_send![split_controller as Id, view];
            if !split_view.is_null() {
                let _: () = msg_send![split_view, setFrame: content_frame];
            }
        }
    }
}

fn update_voice_chat_status_impl(status: &str) {
    let mut state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
    let trimmed = status.trim();
    state.status_base_text = if trimmed.is_empty() {
        "Ready".to_string()
    } else {
        trimmed.to_string()
    };
    state.status_text = compose_runtime_status_text(
        &state.status_base_text,
        state.is_agent_degraded,
        state.runtime_degraded_reason.as_deref(),
    );
    let next_kind = status_kind_for_runtime(&state.status_base_text, state.is_agent_degraded);
    state.status_kind = next_kind;
    apply_status_pill(&state);
    let _ = crate::tray::update_tray_status(next_kind.to_tray());
}

fn set_voice_chat_runtime_degraded_impl(is_degraded: bool, reason: Option<String>) {
    let mut state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
    state.runtime_degraded = is_degraded;
    state.is_agent_degraded = is_degraded;
    state.runtime_degraded_reason = if is_degraded {
        reason.and_then(|text| {
            let trimmed = text.trim();
            if trimmed.is_empty() {
                None
            } else {
                Some(trimmed.to_string())
            }
        })
    } else {
        None
    };
    state.status_text = compose_runtime_status_text(
        &state.status_base_text,
        state.is_agent_degraded,
        state.runtime_degraded_reason.as_deref(),
    );
    state.status_kind = status_kind_for_runtime(&state.status_base_text, state.is_agent_degraded);
    apply_status_pill(&state);
    let _ = crate::tray::update_tray_status(state.status_kind.to_tray());
}

fn status_kind_for_runtime(base_status: &str, runtime_degraded: bool) -> UiStatus {
    if runtime_degraded {
        UiStatus::Error
    } else {
        status_from_detail(base_status)
    }
}

fn compose_runtime_status_text(
    base_status: &str,
    runtime_degraded: bool,
    reason: Option<&str>,
) -> String {
    let base = base_status.trim();
    if !runtime_degraded {
        if base.is_empty() {
            "Ready".to_string()
        } else {
            base.to_string()
        }
    } else if base.is_empty() {
        "Runtime degraded (legacy fallback active)".to_string()
    } else if let Some(reason) = reason.filter(|text| !text.trim().is_empty()) {
        format!("{base} • Runtime degraded ({})", reason.trim())
    } else {
        format!("{base} • Runtime degraded (legacy fallback active)")
    }
}

fn update_voice_chat_context_summary_impl(summary: &str) {
    let mut state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
    state.context_text = summary.to_string();
    apply_status_pill(&state);
}

fn apply_status_pill(state: &VoiceChatOverlayState) {
    unsafe {
        let Some(pill_ptr) = state.status_pill else {
            return;
        };
        let palette = state.status_kind.palette();
        let pill = pill_ptr as Id;
        let layer: Id = msg_send![pill, layer];
        if !layer.is_null() {
            let bg = ui_colors::panel_bg();
            let cg: Id = msg_send![bg, CGColor];
            let _: () = msg_send![layer, setBackgroundColor: cg];
            let border = ui_colors::header_border();
            let cg_border: Id = msg_send![border, CGColor];
            let _: () = msg_send![layer, setBorderColor: cg_border];
            let _: () = msg_send![layer, setBorderWidth: ui_tokens::SURFACE_BORDER_WIDTH];
        }

        if let Some(label_ptr) = state.status_pill_label {
            let label = label_ptr as Id;
            let _: () = msg_send![label, setStringValue: ns_string(state.status_kind.label())];
            let text_color = ui_colors::bubble_meta_text();
            let _: () = msg_send![label, setTextColor: text_color];
        }

        if let Some(dot_ptr) = state.status_pill_dot {
            let dot = dot_ptr as Id;
            let dot_layer: Id = msg_send![dot, layer];
            if !dot_layer.is_null() {
                let dot_color = color_rgba(palette.dot.0, palette.dot.1, palette.dot.2, 0.92);
                let cg: Id = msg_send![dot_color, CGColor];
                let _: () = msg_send![dot_layer, setBackgroundColor: cg];
                // Pulse animation for Listening state
                let pulse_key = ns_string("pulse");
                if state.status_kind == UiStatus::Listening {
                    let existing: Id = msg_send![dot_layer, animationForKey: pulse_key];
                    if existing.is_null() {
                        let ca_anim = Class::get("CABasicAnimation").unwrap();
                        let anim: Id =
                            msg_send![ca_anim, animationWithKeyPath: ns_string("opacity")];
                        let from_val: Id =
                            msg_send![Class::get("NSNumber").unwrap(), numberWithFloat: 0.95f32];
                        let to_val: Id =
                            msg_send![Class::get("NSNumber").unwrap(), numberWithFloat: 0.55f32];
                        let _: () = msg_send![anim, setFromValue: from_val];
                        let _: () = msg_send![anim, setToValue: to_val];
                        let _: () = msg_send![anim, setDuration: 1.0f64];
                        let _: () = msg_send![anim, setAutoreverses: true];
                        let _: () = msg_send![anim, setRepeatCount: f32::INFINITY];
                        let _: () = msg_send![dot_layer, addAnimation: anim forKey: pulse_key];
                    }
                } else {
                    let _: () = msg_send![dot_layer, removeAnimationForKey: pulse_key];
                }
            }
        }

        let detail = if state.context_text.trim().is_empty() {
            format!("Status: {}", state.status_text)
        } else {
            format!(
                "Status: {} • {}",
                state.status_text,
                state.context_text.trim()
            )
        };
        set_tooltip(pill, &detail);
    }
}

/// Minimum interval between layout passes during streaming (prevents main-thread saturation).
const DELTA_LAYOUT_THROTTLE: Duration = Duration::from_millis(50);

fn resolve_delta_index(state: &VoiceChatOverlayState, requested: Option<usize>) -> Option<usize> {
    if let Some(idx) = requested
        && idx < state.messages.len()
        && idx < state.agent_bubble_views.len()
    {
        return Some(idx);
    }
    if !state.messages.is_empty() && state.agent_bubble_views.len() == state.messages.len() {
        return Some(state.messages.len() - 1);
    }
    None
}

fn apply_delta_and_layout(state: &mut VoiceChatOverlayState, updated_index: Option<usize>) {
    let now = Instant::now();
    let should_layout = state
        .last_layout_time
        .is_none_or(|t| now.duration_since(t) >= DELTA_LAYOUT_THROTTLE);

    if should_layout {
        state.last_layout_time = Some(now);
        state.layout_pending = false;
        state.pending_delta_index = None;
        let index = resolve_delta_index(state, updated_index);
        if !index
            .map(|idx| try_update_message_view_in_place(state, idx))
            .unwrap_or(false)
        {
            update_chat_view_with_state(state, false);
        }
    } else {
        state.pending_delta_index = updated_index;
        if !state.layout_pending {
            // Schedule a deferred layout so the latest delta is always rendered.
            state.layout_pending = true;
            let remaining =
                DELTA_LAYOUT_THROTTLE - now.duration_since(state.last_layout_time.unwrap_or(now));
            let millis = remaining.as_millis().max(5) as u64;
            std::thread::spawn(move || {
                std::thread::sleep(Duration::from_millis(millis));
                Queue::main().exec_async(|| {
                    let mut state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
                    if state.layout_pending {
                        state.layout_pending = false;
                        state.last_layout_time = Some(Instant::now());
                        let index = resolve_delta_index(&state, state.pending_delta_index);
                        state.pending_delta_index = None;
                        if !index
                            .map(|idx| try_update_message_view_in_place(&mut state, idx))
                            .unwrap_or(false)
                        {
                            update_chat_view_with_state(&mut state, false);
                        }
                    }
                });
            });
        }
    }
}

fn append_voice_chat_user_delta_impl(delta: &str) {
    let mut state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
    ensure_agent_tab_visible(&mut state);
    let idx = get_or_create_streaming_message_index(&mut state, ChatRole::User);
    if let Some(msg) = state.messages.get_mut(idx) {
        codescribe_core::pipeline::contracts::TranscriptDelta::from_raw(delta).apply(&mut msg.text);
        msg.is_streaming = true;
    }
    apply_delta_and_layout(&mut state, Some(idx));
}

fn append_voice_chat_assistant_delta_impl(delta: &str) {
    let mut state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
    ensure_agent_tab_visible(&mut state);
    let idx = get_or_create_streaming_message_index(&mut state, ChatRole::Assistant);
    if let Some(msg) = state.messages.get_mut(idx) {
        codescribe_core::pipeline::contracts::TranscriptDelta::from_raw(delta).apply(&mut msg.text);
        msg.is_streaming = true;
    }
    apply_delta_and_layout(&mut state, Some(idx));
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

fn message_mode_label(state: &VoiceChatOverlayState) -> String {
    if !matches!(state.conversation_state, ConversationModeState::Inactive) {
        "Moshi".to_string()
    } else if state.auto_send_enabled {
        "AI".to_string()
    } else {
        "Manual".to_string()
    }
}

fn message_role_label(role: ChatRole) -> &'static str {
    match role {
        ChatRole::User => "You",
        ChatRole::Assistant => "Assistant",
        ChatRole::System => "System",
    }
}

fn message_metadata(message: &ChatMessage) -> String {
    let when: DateTime<Local> = message.timestamp.into();
    let time = when.format("%H:%M").to_string();
    let role = message_role_label(message.role);
    if let Some(mode) = message.mode.as_ref() {
        format!("{role} · {time} · {mode}")
    } else {
        format!("{role} · {time}")
    }
}

unsafe fn agent_max_width(state: &VoiceChatOverlayState) -> f64 {
    let width = state
        .agent_scroll_view
        .map(|p| {
            let scroll_view = p as Id;
            let content_view: Id = msg_send![scroll_view, contentView];
            if content_view.is_null() {
                let frame: CGRect = msg_send![scroll_view, frame];
                frame.size.width
            } else {
                let bounds: CGRect = msg_send![content_view, bounds];
                bounds.size.width
            }
        })
        .or_else(|| {
            state.window.map(|p| {
                let window = p as Id;
                let frame: CGRect = msg_send![window, frame];
                (frame.size.width - 32.0).max(240.0)
            })
        })
        .unwrap_or(390.0);

    width.max(240.0)
}

unsafe fn sync_agent_document_view_size(state: &VoiceChatOverlayState, max_width: f64) {
    let Some(container_ptr) = state.agent_container else {
        return;
    };
    let container = container_ptr as Id;

    // Ensure arrangedSubviews frames are up-to-date before measuring.
    let _: () = msg_send![container, setNeedsLayout: true];
    let _: () = msg_send![container, layoutSubtreeIfNeeded];

    // Prefer AppKit's own fittingSize; it accounts for stack layout and avoids cases where
    // arrangedSubviews frames lag behind until interaction (which would make scroll "dead").
    let fitting: CGSize = msg_send![container, fittingSize];
    let mut total_h = fitting.height.max(1.0);

    // Defensive: also sum arranged subview heights + spacing, and take the max.
    let arranged: Id = msg_send![container, arrangedSubviews];
    if !arranged.is_null() {
        let count: usize = msg_send![arranged, count];
        let spacing: f64 = msg_send![container, spacing];
        let mut sum_h = 0.0;
        for i in 0..count {
            let v: Id = msg_send![arranged, objectAtIndex: i];
            if v.is_null() {
                continue;
            }
            let frame: CGRect = msg_send![v, frame];
            sum_h += frame.size.height.max(0.0);
            if i + 1 < count {
                sum_h += spacing;
            }
        }
        total_h = total_h.max(sum_h.max(1.0));
    }

    let _: () = msg_send![container, setFrameSize: CGSize::new(max_width, total_h)];
    let _: () = msg_send![container, setNeedsLayout: true];
    let _: () = msg_send![container, layoutSubtreeIfNeeded];

    // Ensure the scroll view updates its clip view after the document view changes size.
    if let Some(scroll_view_ptr) = state.agent_scroll_view {
        let scroll_view = scroll_view_ptr as Id;
        let _: () = msg_send![scroll_view, tile];
        let content_view: Id = msg_send![scroll_view, contentView];
        if !content_view.is_null() {
            let _: () = msg_send![scroll_view, reflectScrolledClipView: content_view];
        }
    }
}

fn try_update_message_view_in_place(state: &mut VoiceChatOverlayState, index: usize) -> bool {
    unsafe {
        // If the view list doesn't match messages, a full rebuild is safer.
        if state.agent_bubble_views.len() != state.messages.len() {
            return false;
        }
        if index >= state.messages.len() {
            return false;
        }

        let message = &state.messages[index];
        let (bubble_ptr, label_ptr) = state.agent_bubble_views[index];

        let container = bubble_ptr as Id;
        let label = label_ptr as Id;
        let bubble_role = match message.role {
            ChatRole::User => BubbleRole::User,
            ChatRole::Assistant => BubbleRole::Assistant,
            ChatRole::System => BubbleRole::System,
        };
        update_bubble_text(
            label,
            &message.text,
            bubble_role,
            message.is_streaming,
            message.is_error,
        );
        let display_text = display_text_for_message(message);
        resize_bubble_container_for_text(container, label, &display_text);
        let max_width = agent_max_width(state);
        sync_agent_document_view_size(state, max_width);

        // Keep the latest message in view while streaming.
        if index + 1 == state.agent_bubble_views.len()
            && let Some(scroll_view_ptr) = state.agent_scroll_view
        {
            let _ = scroll_view_ptr;
            let bounds: CGRect = msg_send![container, bounds];
            // Scroll to the bottom edge of the bubble; if the bubble is taller than the viewport,
            // scrolling the whole bounds can keep the top visible and never reach the bottom.
            let y = (bounds.size.height - 2.0).max(0.0);
            let rect = CGRect::new(&CGPoint::new(0.0, y), &CGSize::new(bounds.size.width, 2.0));
            let _: () = msg_send![container, scrollRectToVisible: rect];
        }
        true
    }
}

fn finalize_user_message_impl(text: &str) {
    let mut state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
    ensure_agent_tab_visible(&mut state);
    let idx = if let Some(idx) = state.active_user_stream_index.take() {
        if is_valid_stream_message(&state, idx, ChatRole::User) {
            idx
        } else {
            let mode = message_mode_label(&state);
            state.messages.push(ChatMessage {
                role: ChatRole::User,
                text: String::new(),
                is_streaming: false,
                is_error: false,
                timestamp: SystemTime::now(),
                mode: Some(mode),
            });
            state.messages.len() - 1
        }
    } else {
        let mode = message_mode_label(&state);
        state.messages.push(ChatMessage {
            role: ChatRole::User,
            text: String::new(),
            is_streaming: false,
            is_error: false,
            timestamp: SystemTime::now(),
            mode: Some(mode),
        });
        state.messages.len() - 1
    };
    if let Some(msg) = state.messages.get_mut(idx) {
        msg.text = text.to_string();
        msg.is_streaming = false;
        msg.is_error = false;
    }
    update_chat_view_with_state(&mut state, true);
}

fn finalize_user_message_state_only_impl() {
    let mut state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
    let Some(idx) = state
        .active_user_stream_index
        .take()
        .filter(|idx| is_valid_stream_message(&state, *idx, ChatRole::User))
    else {
        return;
    };
    if let Some(last) = state.messages.get_mut(idx) {
        last.is_streaming = false;
        last.is_error = false;
    }
    update_chat_view_with_state(&mut state, true);
}

fn finalize_assistant_message_impl(text: &str, is_error: bool) {
    let mut state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
    ensure_agent_tab_visible(&mut state);
    let idx = if let Some(idx) = state.active_assistant_stream_index.take() {
        if is_valid_stream_message(&state, idx, ChatRole::Assistant) {
            idx
        } else {
            let mode = message_mode_label(&state);
            state.messages.push(ChatMessage {
                role: ChatRole::Assistant,
                text: String::new(),
                is_streaming: false,
                is_error,
                timestamp: SystemTime::now(),
                mode: Some(mode),
            });
            state.messages.len() - 1
        }
    } else {
        let mode = message_mode_label(&state);
        state.messages.push(ChatMessage {
            role: ChatRole::Assistant,
            text: String::new(),
            is_streaming: false,
            is_error,
            timestamp: SystemTime::now(),
            mode: Some(mode),
        });
        state.messages.len() - 1
    };
    if let Some(msg) = state.messages.get_mut(idx) {
        msg.text = text.to_string();
        msg.is_streaming = false;
        msg.is_error = is_error;
    }
    state.is_sending = false;
    update_chat_view_with_state(&mut state, true);
    update_send_button_with_state(&mut state);
}

fn finalize_assistant_message_state_only_impl(is_error: bool) {
    let mut state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
    let Some(idx) = state
        .active_assistant_stream_index
        .take()
        .filter(|idx| is_valid_stream_message(&state, *idx, ChatRole::Assistant))
    else {
        return;
    };
    if let Some(last) = state.messages.get_mut(idx) {
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
        if state.active_tab != Tab::Agent {
            update_active_tab_locked(state, Tab::Agent);
        }
    }
}

fn handoff_transcript_to_chat_impl(transcript: &str) {
    let callback = {
        let mut state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
        ensure_agent_tab_visible(&mut state);
        state.active_user_stream_index = None;
        let mode = message_mode_label(&state);
        state.messages.push(ChatMessage {
            role: ChatRole::User,
            text: transcript.to_string(),
            is_streaming: false,
            is_error: false,
            timestamp: SystemTime::now(),
            mode: Some(mode),
        });
        state.is_sending = true;
        update_chat_view_with_state(&mut state, true);
        update_send_button_with_state(&mut state);

        let handler_guard = SEND_CALLBACK.lock().unwrap_or_else(|e| e.into_inner());
        handler_guard.clone()
    };

    if let Some(handler) = callback {
        handler(transcript.to_string());
    } else {
        let mut state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
        state.is_sending = false;
        update_send_button_with_state(&mut state);
        warn!("No voice-chat send callback set; transcript handoff kept as user message");
    }
}

pub(super) fn clear_voice_chat_text_impl() {
    let btn_ptr = {
        let mut state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
        state.messages.clear();
        state.active_user_stream_index = None;
        state.active_assistant_stream_index = None;
        state.manual_draft.clear();
        state.is_sending = false;
        state.attachments.clear();
        state.attachments_last_sent = None;
        render_attachment_chips_locked(&mut state);
        let btn_ptr = state.agent_attach_button;

        if let Some(input_view) = state.agent_input_text_view {
            unsafe { set_text_view_string(input_view as Id, "") };
        } else if let Some(input_field) = state.agent_input_field {
            unsafe { set_text_field_string(input_field as Id, "") };
        }
        resize_agent_input_locked(&mut state);

        update_chat_view_with_state(&mut state, true);
        update_send_button_with_state(&mut state);
        btn_ptr
    };
    update_attach_button_ui(btn_ptr, 0, Vec::new());
}

/// Send the draft message (called from handlers)
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

        // Check handler BEFORE mutating state to avoid phantom messages
        // when no connector is registered.
        let handler_guard = SEND_CALLBACK.lock().unwrap_or_else(|e| e.into_inner());
        let Some(handler) = handler_guard.clone() else {
            // No send handler — leave state untouched so draft remains in input.
            return;
        };
        drop(handler_guard);

        let attachments_to_send = attachment_should_include_locked(&state);
        // Commit fingerprint under same lock to prevent race with concurrent attachment changes.
        if let Some((fp, _, _)) = attachments_to_send.as_ref() {
            state.attachments_last_sent = Some(*fp);
        }
        if let Some((_fingerprint, _paths, summary)) = attachments_to_send.as_ref() {
            let mode = message_mode_label(&state);
            state.messages.push(ChatMessage {
                role: ChatRole::System,
                text: format!("Attachments (sent once): {}", summary),
                is_streaming: false,
                is_error: false,
                timestamp: SystemTime::now(),
                mode: Some(mode),
            });
        }

        let mode = message_mode_label(&state);
        state.messages.push(ChatMessage {
            role: ChatRole::User,
            text: draft.clone(),
            is_streaming: false,
            is_error: false,
            timestamp: SystemTime::now(),
            mode: Some(mode),
        });
        state.manual_draft.clear();
        state.is_sending = true;
        if let Some(text_view) = state.agent_input_text_view {
            unsafe { set_text_view_string(text_view as Id, "") };
        } else if let Some(input_field) = state.agent_input_field {
            unsafe { set_text_field_string(input_field as Id, "") };
        }
        resize_agent_input_locked(&mut state);
        update_chat_view_with_state(&mut state, true);
        update_send_button_with_state(&mut state);
        (handler, draft, attachments_to_send)
    };

    let (handler, draft, attachments_to_send) = callback;
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

        // Check handler BEFORE mutating state to avoid phantom messages.
        let handler_guard = SEND_CALLBACK.lock().unwrap_or_else(|e| e.into_inner());
        let Some(handler) = handler_guard.clone() else {
            return;
        };
        drop(handler_guard);

        let attachments_to_send = attachment_should_include_locked(&state);
        // Commit fingerprint under same lock to prevent race with concurrent attachment changes.
        if let Some((fp, _, _)) = attachments_to_send.as_ref() {
            state.attachments_last_sent = Some(*fp);
        }
        if let Some((_fingerprint, _paths, summary)) = attachments_to_send.as_ref() {
            let mode = message_mode_label(&state);
            state.messages.push(ChatMessage {
                role: ChatRole::System,
                text: format!("Attachments (sent once): {}", summary),
                is_streaming: false,
                is_error: false,
                timestamp: SystemTime::now(),
                mode: Some(mode),
            });
        }
        state.is_sending = true;
        update_chat_view_with_state(&mut state, true);
        update_send_button_with_state(&mut state);
        (handler, text, attachments_to_send)
    };

    let (handler, text, attachments_to_send) = callback;
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
}

pub(super) fn discard_last_message_impl() {
    let mut state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
    if state.messages.pop().is_some() {
        if let Some(idx) = state.active_user_stream_index
            && idx >= state.messages.len()
        {
            state.active_user_stream_index = None;
        }
        if let Some(idx) = state.active_assistant_stream_index
            && idx >= state.messages.len()
        {
            state.active_assistant_stream_index = None;
        }
        update_chat_view_with_state(&mut state, true);
    }
}

fn active_stream_index_mut(
    state: &mut VoiceChatOverlayState,
    role: ChatRole,
) -> Option<&mut Option<usize>> {
    match role {
        ChatRole::User => Some(&mut state.active_user_stream_index),
        ChatRole::Assistant => Some(&mut state.active_assistant_stream_index),
        ChatRole::System => None,
    }
}

fn active_stream_index(state: &VoiceChatOverlayState, role: ChatRole) -> Option<usize> {
    match role {
        ChatRole::User => state.active_user_stream_index,
        ChatRole::Assistant => state.active_assistant_stream_index,
        ChatRole::System => None,
    }
}

fn is_valid_stream_message(state: &VoiceChatOverlayState, idx: usize, role: ChatRole) -> bool {
    state
        .messages
        .get(idx)
        .map(|msg| msg.role == role && msg.is_streaming)
        .unwrap_or(false)
}

fn get_or_create_streaming_message_index(
    state: &mut VoiceChatOverlayState,
    role: ChatRole,
) -> usize {
    if let Some(idx) = active_stream_index(state, role)
        && is_valid_stream_message(state, idx, role)
    {
        return idx;
    }

    let mode = message_mode_label(state);
    state.messages.push(ChatMessage {
        role,
        text: String::new(),
        is_streaming: true,
        is_error: false,
        timestamp: SystemTime::now(),
        mode: Some(mode),
    });
    let idx = state.messages.len() - 1;
    if let Some(active_idx) = active_stream_index_mut(state, role) {
        *active_idx = Some(idx);
    }
    idx
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

        // Size bubbles to the current visible content width (supports resizable overlay).
        let max_width = agent_max_width(state);
        let zoom = state.zoom_level;
        let base_font = ui_tokens::BODY_FONT_SIZE;

        // Empty state CTA when no messages exist yet.
        if state.messages.is_empty() {
            let empty_label = create_label(LabelConfig {
                frame: CGRect::new(&CGPoint::new(0.0, 0.0), &CGSize::new(max_width, 60.0)),
                text: "Start a conversation\nPress your configured hotkey to record \u{2022} Type to send"
                    .to_string(),
                font_size: base_font * zoom,
                text_color: color_secondary_label(),
                ..Default::default()
            });
            let _: () = msg_send![empty_label, setAlignment: 1_isize]; // NSTextAlignmentCenter
            stack_view_add(container, empty_label);
        }

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
                max_width,
                font_size: base_font * zoom,
                is_streaming: message.is_streaming,
                is_error: message.is_error,
                metadata: Some(message_metadata(message)),
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
        // be disabled and long messages will just "grow" out of view.
        sync_agent_document_view_size(state, max_width);

        if scroll_to_bottom {
            if let Some(bubble) = last_bubble {
                let bounds: CGRect = msg_send![bubble, bounds];
                let y = (bounds.size.height - 2.0).max(0.0);
                let rect = CGRect::new(&CGPoint::new(0.0, y), &CGSize::new(bounds.size.width, 2.0));
                let _: () = msg_send![bubble, scrollRectToVisible: rect];
            } else if let Some(scroll_view_ptr) = state.agent_scroll_view {
                let scroll_view = scroll_view_ptr as Id;
                let content_view: Id = msg_send![scroll_view, contentView];
                let _: () = msg_send![content_view, scrollToPoint: CGPoint::new(0.0, 0.0)];
                let _: () = msg_send![scroll_view, reflectScrolledClipView: content_view];
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
            let symbol = if state.is_sending {
                "ellipsis.circle"
            } else {
                "arrow.up.circle.fill"
            };
            let has_symbol = crate::ui_helpers::set_button_symbol(btn, symbol);
            let title = if has_symbol {
                ""
            } else if state.is_sending {
                "…"
            } else {
                "Send"
            };
            let _: () = msg_send![btn, setTitle: ns_string(title)];
        }
    }
}

pub(super) fn update_attach_button_ui(
    btn_ptr: Option<usize>,
    count: usize,
    mut names: Vec<String>,
) {
    unsafe {
        let Some(btn_ptr) = btn_ptr else {
            return;
        };
        let btn = btn_ptr as Id;
        let has_symbol = crate::ui_helpers::set_button_symbol(btn, "paperclip");
        let title = if count == 0 {
            if has_symbol {
                String::new()
            } else {
                "Attach".to_string()
            }
        } else if has_symbol {
            String::new()
        } else {
            count.to_string()
        };
        let _: () = msg_send![btn, setTitle: ns_string(&title)];

        if count == 0 {
            crate::ui_helpers::set_tooltip(btn, "Attach files (assistant context)");
        } else {
            names.sort();
            let shown: Vec<String> = names.into_iter().take(3).collect();
            let suffix = if count > 3 { "…" } else { "" };
            let tip = format!("Attached: {}{}", shown.join(", "), suffix);
            let _: () = msg_send![btn, setToolTip: ns_string(&tip)];
        }
    }
}

fn attachment_should_include_locked(
    state: &VoiceChatOverlayState,
) -> Option<(u64, Vec<std::path::PathBuf>, String)> {
    if state.attachments.is_empty() {
        return None;
    }
    let fingerprint = attachment_fingerprint(&state.attachments);
    if state.attachments_last_sent == Some(fingerprint) {
        return None;
    }
    let summary = attachment_summary(&state.attachments);
    let paths = Attachment::paths(&state.attachments);
    Some((fingerprint, paths, summary))
}

fn attachment_summary(attachments: &[Attachment]) -> String {
    let mut names: Vec<String> = attachments.iter().map(|a| a.display_name.clone()).collect();
    names.sort();
    if names.len() <= 3 {
        names.join(", ")
    } else {
        format!("{}, … (+{})", names[..3].join(", "), names.len() - 3)
    }
}

fn attachment_fingerprint(attachments: &[Attachment]) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    for a in attachments {
        a.path.hash(&mut hasher);
        if let Ok(meta) = std::fs::metadata(&a.path) {
            meta.len().hash(&mut hasher);
            meta.modified().ok().hash(&mut hasher);
        }
    }
    hasher.finish()
}

// ═══════════════════════════════════════════════════════════
// Attachment Chip Strip
// ═══════════════════════════════════════════════════════════

const CHIP_STRIP_HEIGHT: f64 = 36.0;

/// Remove an attachment by index, re-render chips, update button.
pub fn remove_attachment_at(index: usize) {
    let (btn_ptr, count, names) = {
        let mut state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
        if index < state.attachments.len() {
            state.attachments.remove(index);
            state.attachments_last_sent = None;
        }
        let names: Vec<String> = state
            .attachments
            .iter()
            .map(|a| a.display_name.clone())
            .collect();
        render_attachment_chips_locked(&mut state);
        (state.agent_attach_button, state.attachments.len(), names)
    };
    update_attach_button_ui(btn_ptr, count, names);
}

/// Rebuild chip strip views from current attachments.
/// Must be called from the main thread.
pub(super) fn render_attachment_chips(state: &mut VoiceChatOverlayState) {
    render_attachment_chips_locked(state);
}

fn render_attachment_chips_locked(state: &mut VoiceChatOverlayState) {
    unsafe {
        let Some(strip_ptr) = state.attachment_chip_strip else {
            return;
        };
        let strip = strip_ptr as Id;

        // Get the stack view (document view of the scroll view).
        let stack: Id = msg_send![strip, documentView];
        if stack.is_null() {
            return;
        }

        // Clear existing chips.
        let arranged: Id = msg_send![stack, arrangedSubviews];
        let old_count: usize = msg_send![arranged, count];
        for i in (0..old_count).rev() {
            let view: Id = msg_send![arranged, objectAtIndex: i];
            let _: () = msg_send![stack, removeArrangedSubview: view];
            let _: () = msg_send![view, removeFromSuperview];
        }

        let has_attachments = !state.attachments.is_empty();
        let handler_ptr = match state.action_handler {
            Some(p) => p as Id,
            None => std::ptr::null_mut::<Object>(),
        };

        if has_attachments {
            let mut total_width = 0.0f64;
            for (idx, attachment) in state.attachments.iter().enumerate() {
                let chip = create_chip_view(idx, &attachment.chip_label(20), handler_ptr);
                let _: () = msg_send![stack, addArrangedSubview: chip];
                let chip_frame: CGRect = msg_send![chip, frame];
                total_width += chip_frame.size.width + 6.0;
            }
            // Size the stack view to fit all chips (enables horizontal scrolling).
            let strip_frame: CGRect = msg_send![strip, frame];
            let stack_frame = CGRect::new(
                &CGPoint::new(0.0, 0.0),
                &CGSize::new(total_width.max(strip_frame.size.width), CHIP_STRIP_HEIGHT),
            );
            let _: () = msg_send![stack, setFrame: stack_frame];
        }

        // Show/hide the strip.
        let currently_hidden: bool = msg_send![strip, isHidden];
        if currently_hidden == has_attachments {
            let _: () = msg_send![strip, setHidden: !has_attachments];
        }
    }
    // Reflow layout to account for chip strip height change.
    resize_agent_input_locked(state);
}

/// Create a single chip view: a styled button with the attachment name.
///
/// # Safety
/// Requires main thread.
unsafe fn create_chip_view(index: usize, label: &str, handler: Id) -> Id {
    let ns_button = Class::get("NSButton").unwrap();

    // Measure text width (approximate: 7px per char + padding).
    let text_width = (label.chars().count() as f64 * 7.0).clamp(40.0, 180.0);
    let chip_width = text_width + 24.0; // padding

    let frame = CGRect::new(&CGPoint::new(0.0, 4.0), &CGSize::new(chip_width, 28.0));
    let btn: Id = msg_send![ns_button, alloc];
    let btn: Id = msg_send![btn, initWithFrame: frame];
    let _: () = msg_send![btn, setTitle: ns_string(label)];
    // NSBezelStyleInline = 15 (compact rounded)
    let _: () = msg_send![btn, setBezelStyle: 15i64];
    let _: () = msg_send![btn, setControlSize: 1i64]; // NSControlSizeSmall
    let ns_font = Class::get("NSFont").unwrap();
    let font: Id = msg_send![ns_font, systemFontOfSize: 11.0f64];
    let _: () = msg_send![btn, setFont: font];
    let _: () = msg_send![btn, setTag: index as isize];
    if !handler.is_null() {
        let _: () = msg_send![btn, setTarget: handler];
        let _: () = msg_send![btn, setAction: sel!(onChipClick:)];
    }
    let _: () = msg_send![btn, setTranslatesAutoresizingMaskIntoConstraints: false];

    // Height constraint.
    let height_anchor: Id = msg_send![btn, heightAnchor];
    let constraint: Id = msg_send![height_anchor, constraintEqualToConstant: 28.0f64];
    let _: () = msg_send![constraint, setActive: true];

    btn
}

fn build_attachments_block(paths: &[std::path::PathBuf]) -> String {
    use std::io::Read;
    use std::path::{Path, PathBuf};
    use std::process::Command;

    const MAX_TOTAL_CHARS: usize = 120_000;
    const MAX_FILE_CHARS: usize = 40_000;
    const MAX_FILE_BYTES: usize = 512 * 1024; // cap IO; we only inline a prefix anyway
    const PDF_MIN_TEXT_CHARS: usize = 100;

    fn env_usize(key: &str, default_value: usize) -> usize {
        std::env::var(key)
            .ok()
            .and_then(|v| v.trim().parse::<usize>().ok())
            .filter(|v| *v > 0)
            .unwrap_or(default_value)
    }

    fn env_bool(key: &str, default_value: bool) -> bool {
        std::env::var(key)
            .ok()
            .map(|v| {
                !matches!(
                    v.trim().to_ascii_lowercase().as_str(),
                    "0" | "false" | "no" | "off"
                )
            })
            .unwrap_or(default_value)
    }

    fn tool_env_key(name: &str) -> String {
        format!(
            "CODESCRIBE_TOOL_{}",
            name.to_ascii_uppercase().replace('-', "_")
        )
    }

    fn tool_path(name: &str) -> Option<PathBuf> {
        if let Ok(v) = std::env::var(tool_env_key(name)) {
            let v = v.trim();
            if !v.is_empty() {
                let p = PathBuf::from(v);
                if p.is_file() {
                    return Some(p);
                }
            }
        }

        // macOS GUI apps often have a minimal PATH that doesn't include Homebrew.
        // Prefer common install locations so this works when launched from Finder.
        for dir in ["/opt/homebrew/bin", "/usr/local/bin", "/usr/bin", "/bin"] {
            let p = Path::new(dir).join(name);
            if p.is_file() {
                return Some(p);
            }
        }

        None
    }

    fn tool_command(name: &str) -> Command {
        if let Some(p) = tool_path(name) {
            Command::new(p)
        } else {
            Command::new(name)
        }
    }

    fn command_exists(name: &str) -> bool {
        tool_path(name).is_some()
    }

    fn run_command_stdout(mut cmd: Command) -> Result<Vec<u8>, String> {
        let output = cmd.output().map_err(|e| e.to_string())?;
        if output.status.success() {
            Ok(output.stdout)
        } else {
            let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
            Err(if stderr.is_empty() {
                "command failed".to_string()
            } else {
                stderr
            })
        }
    }

    fn extract_pdf_text_pdftotext(path: &std::path::Path, pages: usize) -> Result<String, String> {
        let pages = pages.max(1);
        let mut cmd = tool_command("pdftotext");
        cmd.args(["-f", "1", "-l", &pages.to_string()])
            .arg(path)
            .arg("-");
        let stdout = run_command_stdout(cmd)?;
        Ok(String::from_utf8_lossy(&stdout).into_owned())
    }

    fn temp_dir(prefix: &str) -> Result<std::path::PathBuf, String> {
        let pid = std::process::id();
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_err(|e| e.to_string())?
            .as_millis();
        let dir = std::env::temp_dir().join(format!("{prefix}_{pid}_{stamp}"));
        std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        Ok(dir)
    }

    fn extract_pdf_text_ocrmypdf(
        path: &std::path::Path,
        pages: usize,
        language: &str,
    ) -> Result<String, String> {
        let pages = pages.max(1);
        let dir = temp_dir("codescribe_pdf_ocr")?;
        let output_pdf = dir.join("ocr.pdf");

        // NOTE: we OCR only first N pages to keep latency acceptable.
        // `ocrmypdf` doesn't support "first N pages" directly, so we pre-split via `pdftk`/`qpdf`
        // would add more deps; instead we run ocrmypdf on whole doc only when asked.
        // Here: best-effort; if you want faster, disable OCR or raise pages and accept cost.
        //
        // We still respect `pages` when extracting with pdftotext after OCR.
        let _ = pages;
        let mut cmd = tool_command("ocrmypdf");
        cmd.args([
            "--language",
            language,
            "--force-ocr",
            "--clean",
            "--deskew",
            "--remove-background",
        ])
        .arg(path)
        .arg(&output_pdf);
        let _ = run_command_stdout(cmd)?;

        let text = extract_pdf_text_pdftotext(&output_pdf, pages).unwrap_or_default();
        let _ = std::fs::remove_dir_all(&dir);
        Ok(text)
    }

    fn extract_pdf_text_tesseract(
        path: &std::path::Path,
        pages: usize,
        language: &str,
    ) -> Result<String, String> {
        let pages = pages.max(1);
        let dir = temp_dir("codescribe_pdf_pages")?;
        let prefix = dir.join("page");

        if command_exists("pdftoppm") {
            let mut cmd = tool_command("pdftoppm");
            cmd.args(["-png", "-r", "300", "-f", "1", "-l", &pages.to_string()])
                .arg(path)
                .arg(&prefix);
            let _ = run_command_stdout(cmd)?;
        } else if command_exists("convert") {
            // ImageMagick fallback: convert first N pages (best-effort).
            let output = dir.join("page-%03d.png");
            let mut cmd = tool_command("convert");
            cmd.args(["-density", "300"])
                .arg(path)
                .args(["-quality", "100"])
                .arg(output);
            let _ = run_command_stdout(cmd)?;
        } else {
            let _ = std::fs::remove_dir_all(&dir);
            return Err("Missing pdftoppm/convert for PDF->image".to_string());
        }

        let mut images: Vec<std::path::PathBuf> = std::fs::read_dir(&dir)
            .map_err(|e| e.to_string())?
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| {
                p.extension()
                    .and_then(|e| e.to_str())
                    .map(|e| matches!(e.to_ascii_lowercase().as_str(), "png" | "jpg" | "jpeg"))
                    .unwrap_or(false)
            })
            .collect();
        images.sort();

        if images.is_empty() {
            let _ = std::fs::remove_dir_all(&dir);
            return Err("PDF->image produced no pages".to_string());
        }

        if !command_exists("tesseract") {
            let _ = std::fs::remove_dir_all(&dir);
            return Err("Missing tesseract".to_string());
        }

        let mut out = String::new();
        for (i, img) in images.iter().take(pages).enumerate() {
            let mut cmd = tool_command("tesseract");
            cmd.arg(img).arg("stdout").args(["-l", language]);
            let stdout = run_command_stdout(cmd)?;
            let text = String::from_utf8_lossy(&stdout);
            out.push_str(&format!("=== PAGE {} ===\n", i + 1));
            out.push_str(text.trim());
            out.push_str("\n\n");
        }

        let _ = std::fs::remove_dir_all(&dir);
        Ok(out)
    }

    fn extract_pdf_text_auto(
        path: &std::path::Path,
        pages: usize,
    ) -> Result<(String, &'static str), String> {
        let text = extract_pdf_text_pdftotext(path, pages).unwrap_or_default();
        if text.trim().chars().count() >= PDF_MIN_TEXT_CHARS {
            return Ok((text, "pdftotext"));
        }

        let ocr_enabled = env_bool("CODESCRIBE_ATTACH_PDF_OCR", true);
        if !ocr_enabled {
            return Ok((text, "pdftotext (minimal text)"));
        }

        fn default_ocr_lang() -> String {
            if let Some(v) = std::env::var("CODESCRIBE_ATTACH_PDF_OCR_LANG")
                .ok()
                .filter(|v| !v.trim().is_empty())
            {
                return v;
            }

            // Prefer Polish+English when available, otherwise fall back to English.
            let mut has_pol = false;
            let mut has_eng = false;
            if command_exists("tesseract") {
                let mut cmd = tool_command("tesseract");
                cmd.arg("--list-langs");
                if let Ok(stdout) = run_command_stdout(cmd) {
                    for line in String::from_utf8_lossy(&stdout).lines() {
                        let l = line.trim();
                        if l == "pol" {
                            has_pol = true;
                        }
                        if l == "eng" {
                            has_eng = true;
                        }
                    }
                }
            }

            if has_pol && has_eng {
                "pol+eng".to_string()
            } else if has_eng {
                "eng".to_string()
            } else if has_pol {
                "pol".to_string()
            } else {
                "eng".to_string()
            }
        }

        let ocr_lang = default_ocr_lang();

        if command_exists("ocrmypdf")
            && command_exists("pdftotext")
            && let Ok(ocr_text) = extract_pdf_text_ocrmypdf(path, pages, &ocr_lang)
            && ocr_text.trim().chars().count() >= PDF_MIN_TEXT_CHARS
        {
            return Ok((ocr_text, "ocrmypdf+pdftotext"));
        }

        if let Ok(ocr_text) = extract_pdf_text_tesseract(path, pages, &ocr_lang)
            && ocr_text.trim().chars().count() >= PDF_MIN_TEXT_CHARS
        {
            return Ok((ocr_text, "tesseract"));
        }

        Ok((text, "pdftotext (minimal text)"))
    }

    fn extract_pdf_quicklook_ocr(
        path: &std::path::Path,
        language: &str,
    ) -> Result<(String, &'static str), String> {
        if !command_exists("qlmanage") {
            return Err("Missing qlmanage".to_string());
        }
        if !command_exists("tesseract") {
            return Err("Missing tesseract".to_string());
        }

        let dir = temp_dir("codescribe_pdf_ql")?;
        let mut cmd = tool_command("qlmanage");
        cmd.args(["-t", "-s", "1400", "-o"]).arg(&dir).arg(path);
        let _ = run_command_stdout(cmd)?;

        let mut images: Vec<std::path::PathBuf> = std::fs::read_dir(&dir)
            .map_err(|e| e.to_string())?
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| {
                p.extension()
                    .and_then(|e| e.to_str())
                    .map(|e| e.eq_ignore_ascii_case("png"))
                    .unwrap_or(false)
            })
            .collect();
        images.sort();

        let Some(img) = images.first() else {
            let _ = std::fs::remove_dir_all(&dir);
            return Err("QuickLook produced no PNG".to_string());
        };

        let mut cmd = tool_command("tesseract");
        cmd.arg(img).arg("stdout").args(["-l", language]);
        let stdout = run_command_stdout(cmd)?;
        let text = String::from_utf8_lossy(&stdout).into_owned();
        let _ = std::fs::remove_dir_all(&dir);
        Ok((text, "quicklook+tesseract"))
    }

    let mut out = String::new();
    out.push_str("ATTACHMENTS (file context)\n");

    let mut total_chars = out.chars().count();
    let mut image_paths: Vec<String> = Vec::new();
    let pdf_pages = env_usize("CODESCRIBE_ATTACH_PDF_PAGES", 3);
    for path in paths {
        if total_chars >= MAX_TOTAL_CHARS {
            break;
        }

        let display = path.to_string_lossy();
        out.push_str("\n---\n");
        out.push_str(&format!("FILE: {display}\n"));

        let ext = path
            .extension()
            .map(|e| e.to_string_lossy().to_ascii_lowercase())
            .unwrap_or_default();
        if ext == "pdf" {
            let ocr_lang = std::env::var("CODESCRIBE_ATTACH_PDF_OCR_LANG")
                .ok()
                .filter(|v| !v.trim().is_empty())
                .unwrap_or_else(|| "pol+eng".to_string());

            let extracted = if command_exists("pdftotext") {
                extract_pdf_text_auto(path, pdf_pages).ok()
            } else {
                // Offline-friendly fallback: render first page via QuickLook and OCR it.
                extract_pdf_quicklook_ocr(path, &ocr_lang).ok()
            };

            match extracted {
                Some((mut text, method)) => {
                    // Cap per-file and total.
                    if text.chars().count() > MAX_FILE_CHARS {
                        text = text.chars().take(MAX_FILE_CHARS).collect();
                        text.push_str("\n… (truncated)\n");
                    }

                    let remaining = MAX_TOTAL_CHARS.saturating_sub(total_chars);
                    if remaining == 0 {
                        break;
                    }
                    let mut snippet: String = text.chars().take(remaining).collect();
                    if snippet.len() < text.len() {
                        snippet.push_str("\n… (truncated)\n");
                    }

                    let pages_hint = if method == "quicklook+tesseract" {
                        "1".to_string()
                    } else {
                        pdf_pages.to_string()
                    };
                    out.push_str(&format!(
                        "(PDF text extracted via {method}; pages: {pages_hint})\n"
                    ));
                    out.push_str("```text\n");
                    out.push_str(&snippet);
                    if !snippet.ends_with('\n') {
                        out.push('\n');
                    }
                    out.push_str("```\n");
                }
                None => {
                    out.push_str(
                        "(PDF: couldn't extract text right now. Quick fix: copy 1-2 pages as text or attach a screenshot (vision).\n\
Tools (optional): `brew install poppler ocrmypdf tesseract-lang`.)\n",
                    );
                }
            }

            total_chars = out.chars().count();
            continue;
        }

        let Ok(mut f) = std::fs::File::open(path) else {
            out.push_str("(failed to open)\n");
            continue;
        };

        let mut buf = Vec::new();
        let _ = (&mut f).take(MAX_FILE_BYTES as u64).read_to_end(&mut buf);

        let Ok(mut s) = String::from_utf8(buf) else {
            let is_image = matches!(
                ext.as_str(),
                "png" | "jpg" | "jpeg" | "webp" | "gif" | "bmp" | "tif" | "tiff"
            );
            if is_image {
                out.push_str("(image detected; will be sent as vision input)\n");
                image_paths.push(display.to_string());
            } else {
                out.push_str("(skipped: not UTF-8 text)\n");
            }
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

    if !image_paths.is_empty() && total_chars < MAX_TOTAL_CHARS {
        out.push_str("\n---\n");
        out.push_str("ATTACHMENTS (image paths)\n");
        image_paths.sort();
        for p in image_paths {
            if total_chars >= MAX_TOTAL_CHARS {
                break;
            }
            out.push_str("- ");
            out.push_str(&p);
            out.push('\n');
            total_chars = out.chars().count();
        }
    }

    out
}

/// Resize the Agent input bar based on current draft text.
///
/// Keeps it compact by default, and grows it when the user types/pastes longer messages.
pub fn resize_agent_input_to_draft() {
    let Ok(mut state) = OVERLAY_STATE.try_lock() else {
        return;
    };
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

        let input_bar = bar_ptr as Id;
        let input_scroll = scroll_ptr as Id;
        let text_view = text_view_ptr as Id;
        let attach_btn = attach_ptr as Id;
        let send_btn = send_ptr as Id;

        let (content_width, content_height) =
            if let Some(container_ptr) = state.split_content_container {
                let container = container_ptr as Id;
                let _: () = msg_send![container, setNeedsLayout: true];
                let _: () = msg_send![container, layoutSubtreeIfNeeded];
                let bounds: CGRect = msg_send![container, bounds];
                (bounds.size.width, bounds.size.height)
            } else {
                let window = window_ptr as Id;
                let window_frame: CGRect = msg_send![window, frame];
                (window_frame.size.width, window_frame.size.height)
            };

        let text = get_text_view_string(text_view);

        // Keep the input compact by default (single-line-ish), then grow smoothly up to a cap.
        let min_h = 44.0;
        let max_h = 180.0;
        let desired_h = if text.trim().is_empty() {
            min_h
        } else {
            // Prefer actual layout height from NSTextView; fall back to a simple heuristic.
            let mut measured: Option<f64> = None;
            let layout: Id = msg_send![text_view, layoutManager];
            let container: Id = msg_send![text_view, textContainer];
            if !layout.is_null() && !container.is_null() {
                let _: () = msg_send![layout, ensureLayoutForTextContainer: container];
                let used: CGRect = msg_send![layout, usedRectForTextContainer: container];
                let text_h = used.size.height.max(0.0);
                measured = Some((text_h + 20.0).clamp(min_h, max_h));
            }

            measured.unwrap_or_else(|| {
                let hard_lines = (text.matches('\n').count() + 1).max(1);
                // Heuristic for wrapped lines: assume ~52 chars per visual line at this width.
                let wrapped_lines = text.chars().count().div_ceil(52).max(1);
                let visual_lines = hard_lines.max(wrapped_lines);
                let line_h = 18.0;
                (min_h + (visual_lines.saturating_sub(1) as f64) * line_h).clamp(min_h, max_h)
            })
        };

        let pad = ui_tokens::EDGE_PADDING_TIGHT;
        let gap = ui_tokens::CONTENT_GAP;
        let input_gap = (gap * 0.5).max(4.0);
        let footer_inset = ui_tokens::FOOTER_INSET;
        let bar_width = (content_width - pad * 2.0).max(120.0);
        let current_bar: CGRect = msg_send![input_bar, frame];
        let height_same = (current_bar.size.height - desired_h).abs() < 0.5;
        let width_same = (current_bar.size.width - bar_width).abs() < 0.5;
        // Check if agent scroll frame needs updating (e.g. chip strip toggled).
        // We compare the actual frame origin against the expected bottom rather
        // than checking visibility flags, because setHidden may have already been
        // called (by render_attachment_chips_locked) before we get here — making
        // the visibility flag look "stable" even though the scroll frame hasn't
        // been adjusted yet.
        let scroll_needs_reflow = if let Some(agent_scroll_ptr) = state.agent_scroll_view {
            let agent_scroll = agent_scroll_ptr as Id;
            let current_frame: CGRect = msg_send![agent_scroll, frame];
            let strip_extra = if let Some(strip_ptr) = state.attachment_chip_strip {
                let strip = strip_ptr as Id;
                let strip_visible: bool = !msg_send![strip, isHidden];
                if strip_visible {
                    CHIP_STRIP_HEIGHT + input_gap
                } else {
                    0.0
                }
            } else {
                0.0
            };
            let expected_bottom = footer_inset + desired_h + input_gap + strip_extra;
            (current_frame.origin.y - expected_bottom).abs() > 0.5
        } else {
            false
        };
        if height_same && width_same && !scroll_needs_reflow {
            return;
        }

        // Resize input bar (anchored to bottom).
        let new_bar_frame = CGRect::new(
            &CGPoint::new(pad, footer_inset),
            &CGSize::new(bar_width, desired_h),
        );
        let _: () = msg_send![input_bar, setFrame: new_bar_frame];

        // Resize the input row (attach left, text center, send right).
        let row_layout = crate::ui_helpers::chat_input_row_layout(bar_width, desired_h);
        let text_area_frame = CGRect::new(
            &CGPoint::new(row_layout.text_x, row_layout.text_y),
            &CGSize::new(row_layout.text_width, row_layout.text_height),
        );
        let _: () = msg_send![input_scroll, setFrame: text_area_frame];

        // Recenter buttons vertically.
        let attach_frame = CGRect::new(
            &CGPoint::new(row_layout.attach_x, row_layout.attach_y),
            &CGSize::new(row_layout.button_width, row_layout.button_height),
        );
        let _: () = msg_send![attach_btn, setFrame: attach_frame];
        let send_frame = CGRect::new(
            &CGPoint::new(row_layout.send_x, row_layout.send_y),
            &CGSize::new(row_layout.button_width, row_layout.button_height),
        );
        let _: () = msg_send![send_btn, setFrame: send_frame];

        // Position chip strip above input bar and resize agent scroll view.
        let chip_strip_extra = if let Some(strip_ptr) = state.attachment_chip_strip {
            let strip = strip_ptr as Id;
            let strip_visible: bool = !msg_send![strip, isHidden];
            if strip_visible {
                let strip_y = footer_inset + desired_h + input_gap;
                let strip_frame = CGRect::new(
                    &CGPoint::new(pad, strip_y),
                    &CGSize::new(bar_width, CHIP_STRIP_HEIGHT),
                );
                let _: () = msg_send![strip, setFrame: strip_frame];
                CHIP_STRIP_HEIGHT + input_gap
            } else {
                0.0
            }
        } else {
            0.0
        };

        // Resize Agent scroll view so it doesn't overlap with input + chips.
        if let Some(agent_scroll_ptr) = state.agent_scroll_view {
            let agent_scroll = agent_scroll_ptr as Id;
            let bottom = footer_inset + desired_h + input_gap + chip_strip_extra;
            let top = content_height - gap;
            let new_agent_frame = CGRect::new(
                &CGPoint::new(pad, bottom),
                &CGSize::new(
                    (content_width - pad * 2.0).max(0.0),
                    (top - bottom).max(0.0),
                ),
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

/// ObjC handles owned by the overlay state via `[cls new]` (+1 retain each).
/// These must receive a balancing `release` exactly once when the overlay is
/// permanently torn down. Subviews (`blur_view`, pills, drawer, etc.) are
/// retained by the window and need no explicit release.
///
/// The reuse path in `voice_chat/mod.rs` and the AppKit `windowWillClose`
/// delegate callback in `voice_chat/handlers.rs` both clear the state without
/// taking ownership of these pointers — they call the lighter
/// `clear_overlay_state` directly. Only `hide_voice_chat_overlay_impl` is
/// authoritative for releasing them, so handle ownership transfer happens
/// here and nowhere else.
struct ReleasedOverlayHandles {
    window_delegate: Option<usize>,
    action_handler: Option<usize>,
    window: Option<usize>,
}

/// Drain the three owned ObjC handles out of the overlay state and clear all
/// other fields. Returns the handles for the caller to release after dropping
/// the state lock. Calling code MUST eventually `release` each `Some(ptr)`
/// exactly once or leak the underlying object.
fn take_handles_and_clear_overlay_state(
    state: &mut VoiceChatOverlayState,
) -> ReleasedOverlayHandles {
    let handles = ReleasedOverlayHandles {
        window_delegate: state.window_delegate.take(),
        action_handler: state.action_handler.take(),
        window: state.window.take(),
    };
    clear_overlay_state(state);
    handles
}

fn hide_voice_chat_overlay_impl() {
    // IMPORTANT: do not hold OVERLAY_STATE while calling `window_close`.
    // `window_close` triggers AppKit notifications/delegate callbacks (windowWillClose),
    // and those callbacks also lock OVERLAY_STATE. Holding the lock here can deadlock
    // the main thread (observed as a hard freeze/hang).
    let handles = {
        let mut state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
        take_handles_and_clear_overlay_state(&mut state)
    };

    if let Some(window_ptr) = handles.window {
        // SAFETY: `window_ptr` was obtained from `[NSWindow alloc] init...]`
        // / `[cls new]` on the main thread and stored in `handles.window`
        // while still retained. We are on the main thread (overlay teardown
        // runs from the AppKit run loop) and the pointer has not yet been
        // released.
        unsafe {
            let window = window_ptr as Id;
            crate::ui_helpers::animate_fade(window, 0.0, 0.15);
            // The shared overlay shell sets `releasedWhenClosed = false`
            // (see `app/ui/shared/helpers.rs`), so `window_close` does NOT
            // balance the +1 retain from window construction. We must
            // `release` the window pointer ourselves below.
            crate::ui_helpers::window_close(window);
        }
    }

    // Balance the +1 retain count from `[cls new]` on each owned handle.
    // Subviews are owned by the window and released transitively.
    // SAFETY: each pointer below was obtained from `[cls new]` (or equivalent
    // alloc/init pair) on the main thread, retained at +1, and is still alive
    // because `take_handles_and_clear_overlay_state` is the unique teardown
    // site. Caller invariants guarantee single-threaded main-thread access.
    unsafe {
        if let Some(ptr) = handles.window_delegate {
            let _: () = msg_send![ptr as Id, release];
        }
        if let Some(ptr) = handles.action_handler {
            let _: () = msg_send![ptr as Id, release];
        }
        if let Some(ptr) = handles.window {
            let _: () = msg_send![ptr as Id, release];
        }
    }

    clear_search_field();
}

/// Reset the overlay state to its default shape. Does NOT release ObjC retains
/// on `window`, `window_delegate`, or `action_handler` — the caller is
/// responsible for that via `take_handles_and_clear_overlay_state` when the
/// overlay is being permanently torn down. This entry point is safe for the
/// reuse path (stale dangling pointers) and the `windowWillClose` callback
/// (release already in flight from `hide_voice_chat_overlay_impl`).
pub fn clear_overlay_state(state: &mut VoiceChatOverlayState) {
    state.window = None;
    state.window_delegate = None;
    state.action_handler = None;
    state.blur_view = None;
    state.split_view_controller = None;
    state.split_sidebar_item = None;
    state.split_content_item = None;
    state.split_sidebar_container = None;
    state.split_content_container = None;
    state.title_label = None;
    state.status_pill = None;
    state.status_pill_label = None;
    state.status_pill_dot = None;
    state.tab_drawer_button = None;
    state.tab_agent_button = None;
    state.tab_settings_button = None;
    state.favorites_button = None;
    state.close_button = None;
    state.drawer_scroll_view = None;
    state.drawer_container = None;
    state.drawer_edge_effect = None;
    state.search_field = None;
    state.search_label = None;
    state.agent_scroll_view = None;
    state.agent_container = None;
    state.agent_bubble_views.clear();
    state.agent_input_bar = None;
    state.agent_input_scroll_view = None;
    state.agent_input_text_view = None;
    state.agent_input_field = None;
    state.agent_attach_button = None;
    state.agent_send_button = None;
    state.attachments.clear();
    state.attachments_last_sent = None;
    state.attachment_chip_strip = None;
    state.active_tab = Tab::Drawer;
    state.pending_tab = None;
    state.active_user_stream_index = None;
    state.active_assistant_stream_index = None;
    state.is_sending = false;
    state.manual_draft.clear();
    state.conversation_state = ConversationModeState::Inactive;
}

fn refresh_drawer_impl() {
    let mut state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
    state.favorites = load_favorites_from_disk();
    let query = drawer_query_from_state(&state);
    state.drawer_entries = load_drawer_entries();
    render_drawer_entries(&mut state, &query);
}

pub fn handle_card_copy(index: usize) {
    let state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(entry) = state.drawer_entries.get(index) {
        if is_drawer_unavailable_placeholder(entry) {
            return;
        }
        match &entry.source {
            DrawerEntrySource::Thread { id } => {
                if let Ok(store) = ThreadStore::new() {
                    if let Ok(thread) = store.load_thread(id) {
                        copy_to_clipboard(&thread_markdown_for_copy(&thread));
                        return;
                    }
                    if let Ok(raw) = std::fs::read_to_string(&entry.path) {
                        copy_to_clipboard(&raw);
                    }
                }
            }
            DrawerEntrySource::LegacyFile => {
                if let Ok(contents) = std::fs::read_to_string(&entry.path) {
                    copy_to_clipboard(&contents);
                }
            }
        }
    }
}

pub fn handle_card_edit(index: usize) {
    let (path, window_usize) = {
        let state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
        let path = state.drawer_entries.get(index).map(|e| e.path.clone());
        (path, state.window)
    };

    let Some(path) = path else {
        return;
    };
    if path.as_os_str().is_empty() {
        return;
    }

    tracing::info!("Drawer Edit clicked: {}", path.display());
    let ok = open_file_in_editor(&path);
    if !ok {
        #[cfg(target_os = "macos")]
        {
            let _ = std::process::Command::new("/usr/bin/open")
                .arg("-R")
                .arg(&path)
                .status();
        }
        tracing::warn!("Drawer Edit failed to open: {}", path.display());
        return;
    }

    // UX: briefly hide the overlay so the editor is visible immediately.
    // Then only bring it back if CodeScribe is still the active app.
    #[cfg(target_os = "macos")]
    if let Some(window_usize) = window_usize {
        unsafe {
            crate::ui_helpers::window_hide(window_usize as Id);
        }

        std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(750));

            Queue::main().exec_async(move || {
                let still_same_window = {
                    let state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
                    state.window == Some(window_usize)
                };
                if !still_same_window {
                    return;
                }

                let is_active = unsafe {
                    let ns_running_app = match Class::get("NSRunningApplication") {
                        Some(c) => c,
                        None => return,
                    };
                    let current: Id = msg_send![ns_running_app, currentApplication];
                    if current.is_null() {
                        return;
                    }
                    let active: bool = msg_send![current, isActive];
                    active
                };

                // Restore floating level and show only if CodeScribe is active.
                unsafe {
                    let window = window_usize as Id;
                    let _: () = msg_send![
                        window,
                        setLevel: crate::ui_helpers::NS_FLOATING_WINDOW_LEVEL
                    ];
                }
                if is_active {
                    unsafe {
                        crate::ui_helpers::window_show(window_usize as Id);
                    }
                }
            });
        });
    }
}

pub fn handle_card_delete(index: usize) {
    let mut state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(entry) = state.drawer_entries.get(index) {
        if is_drawer_unavailable_placeholder(entry) {
            return;
        }
        let favorite_key = drawer_entry_favorite_key(entry);
        match &entry.source {
            DrawerEntrySource::Thread { id } => {
                if let Ok(store) = ThreadStore::new() {
                    if let Err(err) = store.delete_thread(id) {
                        warn!("Failed to delete thread {id}: {err}");
                    }
                } else if let Err(err) = std::fs::remove_file(&entry.path) {
                    warn!(
                        "Failed to delete thread fallback {}: {}",
                        entry.path.display(),
                        err
                    );
                }
            }
            DrawerEntrySource::LegacyFile => {
                if let Err(err) = std::fs::remove_file(&entry.path) {
                    warn!("Failed to delete {}: {}", entry.path.display(), err);
                }
            }
        }
        state.favorites.remove(&favorite_key);
        save_favorites_to_disk(&state.favorites);
    }
    state.favorites = load_favorites_from_disk();
    let query = drawer_query_from_state(&state);
    state.drawer_entries = load_drawer_entries_with_query(&query);
    render_drawer_entries(&mut state, &query);
}

pub fn handle_card_favorite(index: usize) {
    let mut state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
    let Some(entry) = state.drawer_entries.get_mut(index) else {
        return;
    };
    if is_drawer_unavailable_placeholder(entry) {
        return;
    }

    entry.is_favorite = !entry.is_favorite;
    let is_favorite = entry.is_favorite;
    let key = drawer_entry_favorite_key(entry);
    let thread_id = match &entry.source {
        DrawerEntrySource::Thread { id } => Some(id.clone()),
        DrawerEntrySource::LegacyFile => None,
    };

    if is_favorite {
        state.favorites.insert(key);
    } else {
        state.favorites.remove(&key);
    }
    save_favorites_to_disk(&state.favorites);

    if let Some(id) = thread_id
        && let Ok(store) = ThreadStore::new()
        && let Err(err) = store.set_thread_favorite(&id, is_favorite)
    {
        warn!("Failed to update thread favorite {id}: {err}");
    }
    update_favorites_button_with_state(&mut state);
    let query = drawer_query_from_state(&state);
    render_drawer_entries(&mut state, &query);
}

pub(super) fn toggle_drawer_favorites_only_impl() {
    let mut state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
    state.drawer_favorites_only = !state.drawer_favorites_only;

    // Jump to Drawer (this feature is Drawer-scoped).
    update_active_tab_locked(&mut state, Tab::Drawer);

    update_favorites_button_with_state(&mut state);

    let query = drawer_query_from_state(&state);
    render_drawer_entries(&mut state, &query);
}

fn update_favorites_button_with_state(state: &mut VoiceChatOverlayState) {
    unsafe {
        let Some(btn_ptr) = state.favorites_button else {
            return;
        };
        let btn = btn_ptr as Id;
        let symbol = if state.drawer_favorites_only {
            "heart.fill"
        } else {
            "heart"
        };
        let has_symbol = set_button_symbol(btn, symbol);
        if !has_symbol {
            let title = if state.drawer_favorites_only {
                "♥"
            } else {
                "♡"
            };
            let title = ns_string(title);
            let _: () = msg_send![btn, setTitle: title];
        }
    }
}

fn drawer_query_from_state(state: &VoiceChatOverlayState) -> String {
    state
        .search_field
        .map(|field| unsafe { get_text_field_string(field as Id) })
        .unwrap_or_default()
}

fn drawer_entry_matches_query(entry: &DrawerEntry, query_lower: &str) -> bool {
    if query_lower.is_empty() {
        return true;
    }
    let path = entry.path.to_string_lossy();
    let mut haystack =
        String::with_capacity(path.len() + entry.preview.len() + entry.search_corpus.len() + 96);
    haystack.push_str(entry_type_label(entry));
    haystack.push(' ');
    haystack.push_str(mode_label(entry.mode));
    haystack.push(' ');
    haystack.push_str(&path);
    haystack.push(' ');
    if let Some(file_name) = entry.path.file_name().and_then(|name| name.to_str()) {
        haystack.push_str(file_name);
        haystack.push(' ');
    }
    if let DrawerEntrySource::Thread { id } = &entry.source {
        haystack.push_str(id);
        haystack.push(' ');
    }
    haystack.push_str(&entry.preview);
    haystack.push(' ');
    haystack.push_str(&entry.search_corpus);
    haystack.to_lowercase().contains(query_lower)
}

fn filtered_drawer_entries<'a>(
    state: &'a VoiceChatOverlayState,
    query: &str,
) -> Vec<(usize, &'a DrawerEntry)> {
    let filter = query.trim().to_lowercase();
    let mut out = Vec::new();
    for (index, entry) in state.drawer_entries.iter().enumerate() {
        if state.drawer_favorites_only && !entry.is_favorite {
            continue;
        }
        if !drawer_entry_matches_query(entry, &filter) {
            continue;
        }
        out.push((index, entry));
    }
    out
}

fn render_drawer_entries(state: &mut VoiceChatOverlayState, query: &str) {
    unsafe {
        let Some(container_ptr) = state.drawer_container else {
            return;
        };
        let container = container_ptr as Id;
        stack_view_clear(container);

        let visible = filtered_drawer_entries(state, query);
        for (index, entry) in visible.iter() {
            let card = create_drawer_card(entry, *index, state.action_handler, query);
            stack_view_add(container, card);
        }

        if visible.is_empty() {
            let frame: CGRect = msg_send![container, frame];
            let empty_state = create_drawer_empty_state(frame.size.width, state.action_handler);
            stack_view_add(container, empty_state);
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

fn create_drawer_empty_state(width: f64, handler: Option<usize>) -> Id {
    fn overlay_hotkey_shortcuts_tooltip() -> String {
        let (hold, toggle) = super::shortcuts_lines(crate::os::hotkeys::ModeHotkeyBindings::load());
        format!("{hold}\n{toggle}")
    }

    unsafe {
        let ns_view = Class::get("NSView").unwrap();
        let frame = CGRect::new(
            &CGPoint::new(0.0, 0.0),
            &CGSize::new(width.max(240.0), ui_tokens::EMPTY_STATE_HEIGHT),
        );
        let view: Id = msg_send![ns_view, alloc];
        let view: Id = msg_send![view, initWithFrame: frame];
        let _: () = msg_send![view, setWantsLayer: true];
        let layer: Id = msg_send![view, layer];
        if !layer.is_null() {
            let bg = ui_colors::empty_state_bg();
            let cg: Id = msg_send![bg, CGColor];
            let _: () = msg_send![layer, setBackgroundColor: cg];
            apply_tafla_surface(layer, true);
        }

        let pad = ui_tokens::EDGE_PADDING;
        let title = create_label(LabelConfig {
            frame: CGRect::new(
                &CGPoint::new(pad, frame.size.height - 36.0),
                &CGSize::new(frame.size.width - pad * 2.0, 20.0),
            ),
            text: "No items yet".to_string(),
            font_size: ui_tokens::BODY_FONT_SIZE,
            bold: true,
            text_color: color_label(),
            background_color: None,
            selectable: false,
            editable: false,
        });
        add_subview(view, title);

        let body = create_label(LabelConfig {
            frame: CGRect::new(
                &CGPoint::new(pad, frame.size.height - 58.0),
                &CGSize::new(frame.size.width - pad * 2.0, 18.0),
            ),
            text: "Start recording to capture a transcript.".to_string(),
            font_size: ui_tokens::SMALL_FONT_SIZE,
            bold: false,
            text_color: color_secondary_label(),
            background_color: None,
            selectable: false,
            editable: false,
        });
        add_subview(view, body);

        let body2 = create_label(LabelConfig {
            frame: CGRect::new(
                &CGPoint::new(pad, frame.size.height - 76.0),
                &CGSize::new(frame.size.width - pad * 2.0, 18.0),
            ),
            text: "Need permissions or hotkeys? Open Settings.".to_string(),
            font_size: ui_tokens::SMALL_FONT_SIZE,
            bold: false,
            text_color: color_secondary_label(),
            background_color: None,
            selectable: false,
            editable: false,
        });
        add_subview(view, body2);

        let button_h = ui_tokens::EMPTY_STATE_BUTTON_HEIGHT;
        let button_w = ui_tokens::EMPTY_STATE_BUTTON_WIDTH;
        let button_gap = ui_tokens::EMPTY_STATE_BUTTON_GAP;
        let row_w = button_w * 2.0 + button_gap;
        let row_x = ((frame.size.width - row_w) / 2.0).max(pad);

        let start_button = create_button(
            CGRect::new(&CGPoint::new(row_x, pad), &CGSize::new(button_w, button_h)),
            "Start recording",
            button_style::ROUNDED,
        );
        let overlay_button = create_button(
            CGRect::new(
                &CGPoint::new(row_x + button_w + button_gap, pad),
                &CGSize::new(button_w, button_h),
            ),
            "Open Settings",
            button_style::ROUNDED,
        );

        if let Some(handler_ptr) = handler {
            let handler_id = handler_ptr as Id;
            button_set_action(start_button, handler_id, sel!(onStartRecording:));
            button_set_action(overlay_button, handler_id, sel!(onTabSettings:));
        }

        let shortcuts_tooltip = overlay_hotkey_shortcuts_tooltip();
        set_tooltip(start_button, &shortcuts_tooltip);
        set_tooltip(
            overlay_button,
            "Open Settings (permissions, hotkeys, and runtime services)",
        );
        add_subview(view, start_button);
        add_subview(view, overlay_button);

        view
    }
}

fn chat_markdown_from_messages(messages: &[ChatMessage], assistant_only: bool) -> String {
    let exported_at = Local::now().format("%Y-%m-%d %H:%M:%S").to_string();
    let mut out = String::new();
    out.push_str("# CodeScribe Chat Export\n\n");
    out.push_str(&format!("- exported_at: {}\n", exported_at));
    out.push_str(&format!(
        "- scope: {}\n\n",
        if assistant_only {
            "assistant_only"
        } else {
            "all"
        }
    ));

    for msg in messages {
        if assistant_only && msg.role != ChatRole::Assistant {
            continue;
        }
        let role = match msg.role {
            ChatRole::User => "User",
            ChatRole::Assistant => "Assistant",
            ChatRole::System => "System",
        };
        out.push_str(&format!("## {}\n\n", role));
        out.push_str(msg.text.trim_end());
        out.push_str("\n\n");
    }

    out.trim_end().to_string() + "\n"
}

#[cfg(test)]
mod tests {
    use super::*;
    use serial_test::serial;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};

    fn sample_drawer_entry(
        path: &str,
        preview: &str,
        mode: TranscriptionMode,
        is_ai_formatted: bool,
        is_favorite: bool,
    ) -> DrawerEntry {
        let mode_label = match mode {
            TranscriptionMode::Hold => "Ctrl+Hold",
            TranscriptionMode::Assistive => "Shift/Cmd",
            TranscriptionMode::Toggle => "Toggle",
            TranscriptionMode::Conversation => "Moshi",
        };
        let entry_type = if is_ai_formatted { "AI" } else { "Tt" };
        let search_corpus =
            format!("{entry_type} {mode_label} {path} {preview}").to_ascii_lowercase();
        DrawerEntry {
            source: DrawerEntrySource::LegacyFile,
            path: PathBuf::from(path),
            timestamp: SystemTime::now(),
            mode,
            preview: preview.to_string(),
            search_corpus,
            is_ai_formatted,
            is_favorite,
        }
    }

    #[test]
    fn filtered_drawer_entries_matches_preview_path_and_title_case_insensitively() {
        let mut state = VoiceChatOverlayState {
            drawer_entries: vec![
                sample_drawer_entry(
                    "meeting_notes.md",
                    "Follow-up from team sync",
                    TranscriptionMode::Hold,
                    false,
                    false,
                ),
                sample_drawer_entry(
                    "roadmap.md",
                    "Architecture review memo",
                    TranscriptionMode::Assistive,
                    true,
                    true,
                ),
            ],
            ..Default::default()
        };

        assert_eq!(filtered_drawer_entries(&state, "TEAM").len(), 1);
        assert_eq!(filtered_drawer_entries(&state, "MEETING_NOTES").len(), 1);
        assert_eq!(filtered_drawer_entries(&state, "shift/cmd").len(), 1);
        assert_eq!(filtered_drawer_entries(&state, "AI").len(), 1);

        state.drawer_favorites_only = true;
        assert_eq!(filtered_drawer_entries(&state, "").len(), 1);
    }

    #[test]
    fn filtered_drawer_entries_returns_empty_when_query_has_no_match() {
        let state = VoiceChatOverlayState {
            drawer_entries: vec![
                sample_drawer_entry(
                    "draft-a.md",
                    "First transcript snippet",
                    TranscriptionMode::Hold,
                    false,
                    false,
                ),
                sample_drawer_entry(
                    "draft-b.md",
                    "Second transcript snippet",
                    TranscriptionMode::Toggle,
                    false,
                    false,
                ),
            ],
            ..Default::default()
        };

        assert!(filtered_drawer_entries(&state, "missing phrase").is_empty());
    }

    #[test]
    fn filtered_drawer_entries_clear_query_restores_full_list() {
        let state = VoiceChatOverlayState {
            drawer_entries: vec![
                sample_drawer_entry("first.md", "alpha", TranscriptionMode::Hold, false, false),
                sample_drawer_entry(
                    "second.md",
                    "beta",
                    TranscriptionMode::Assistive,
                    false,
                    true,
                ),
            ],
            ..Default::default()
        };

        assert_eq!(filtered_drawer_entries(&state, "alpha").len(), 1);
        assert_eq!(filtered_drawer_entries(&state, "").len(), 2);
        assert_eq!(filtered_drawer_entries(&state, "   ").len(), 2);
    }

    #[test]
    fn filtered_drawer_entries_keeps_original_indices_for_card_actions() {
        let state = VoiceChatOverlayState {
            drawer_entries: vec![
                sample_drawer_entry("first.md", "alpha", TranscriptionMode::Hold, false, false),
                sample_drawer_entry(
                    "second.md",
                    "alpha",
                    TranscriptionMode::Assistive,
                    false,
                    false,
                ),
                sample_drawer_entry("third.md", "alpha", TranscriptionMode::Toggle, false, false),
            ],
            ..Default::default()
        };

        let visible = filtered_drawer_entries(&state, "third");
        assert_eq!(visible.len(), 1);
        assert_eq!(visible[0].0, 2);
    }

    #[test]
    fn filtered_drawer_entries_matches_thread_message_and_note_corpus() {
        let state = VoiceChatOverlayState {
            drawer_entries: vec![DrawerEntry {
                source: DrawerEntrySource::Thread {
                    id: "t_2026-02-23_abc123".to_string(),
                },
                path: PathBuf::from("thread_t_2026-02-23_abc123.json"),
                timestamp: SystemTime::now(),
                mode: TranscriptionMode::Assistive,
                preview: "clinical recap".to_string(),
                search_corpus: "renal values improved call owner tomorrow".to_string(),
                is_ai_formatted: true,
                is_favorite: false,
            }],
            ..Default::default()
        };

        assert_eq!(filtered_drawer_entries(&state, "renal values").len(), 1);
        assert_eq!(filtered_drawer_entries(&state, "call owner").len(), 1);
        assert_eq!(filtered_drawer_entries(&state, "missing phrase").len(), 0);
    }

    #[test]
    fn drawer_unavailable_placeholder_entry_has_expected_metadata() {
        let entry = thread_history_unavailable_drawer_entry();

        assert!(matches!(entry.source, DrawerEntrySource::LegacyFile));
        assert!(entry.path.as_os_str().is_empty());
        assert_eq!(entry.preview, "Thread history unavailable — storage error");
        assert!(!entry.is_ai_formatted);
        assert!(entry.search_corpus.contains("unavailable"));
        assert!(entry.search_corpus.contains("error"));
        assert!(is_drawer_unavailable_placeholder(&entry));
        assert!(drawer_entry_matches_query(&entry, "unavailable"));
        assert!(drawer_entry_matches_query(&entry, "error"));
    }

    #[test]
    #[serial]
    fn runtime_degraded_status_persists_across_status_updates() {
        {
            let mut state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
            *state = VoiceChatOverlayState::default();
        }

        set_voice_chat_runtime_degraded_impl(
            true,
            Some("Legacy formatter fallback is active.".to_string()),
        );
        update_voice_chat_status_impl("Sending...");

        {
            let state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
            assert!(state.runtime_degraded);
            assert!(state.is_agent_degraded);
            assert_eq!(state.status_base_text, "Sending...");
            assert_eq!(state.status_kind, UiStatus::Error);
            assert!(state.status_text.contains("Runtime degraded"));
        }

        set_voice_chat_runtime_degraded_impl(false, None);

        {
            let state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
            assert!(!state.runtime_degraded);
            assert!(!state.is_agent_degraded);
            assert_eq!(state.status_text, "Sending...");
            assert_eq!(state.status_kind, UiStatus::Processing);
        }

        update_voice_chat_status_impl("AI Response:");

        let state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
        assert_eq!(state.status_base_text, "AI Response:");
        assert_eq!(state.status_text, "AI Response:");
        assert_eq!(state.status_kind, UiStatus::Idle);
    }

    #[test]
    fn drawer_entry_subtitle_marks_threadstore_index_only_when_path_missing() {
        let entry = DrawerEntry {
            source: DrawerEntrySource::Thread {
                id: "t_2026-02-23_missing".to_string(),
            },
            path: PathBuf::from("__missing_thread_guardrail_test__.json"),
            timestamp: SystemTime::now(),
            mode: TranscriptionMode::Assistive,
            preview: "summary".to_string(),
            search_corpus: "summary".to_string(),
            is_ai_formatted: true,
            is_favorite: false,
        };

        let subtitle = drawer_entry_subtitle(&entry);
        assert!(subtitle.contains("ThreadStore (index-only)"));
        assert!(subtitle.contains("thread:t_2026-02-23_missing"));
    }

    #[test]
    fn display_text_for_message_handles_streaming() {
        let streaming_empty = ChatMessage {
            role: ChatRole::Assistant,
            text: String::new(),
            is_streaming: true,
            is_error: false,
            timestamp: SystemTime::now(),
            mode: None,
        };
        assert_eq!(display_text_for_message(&streaming_empty), "• • •");

        let streaming = ChatMessage {
            text: "hello".to_string(),
            ..streaming_empty
        };
        assert_eq!(display_text_for_message(&streaming), "hello …");

        let finished = ChatMessage {
            is_streaming: false,
            ..streaming
        };
        assert_eq!(display_text_for_message(&finished), "hello");
    }

    #[test]
    fn update_active_tab_switches_between_drawer_and_agent() {
        let mut state = VoiceChatOverlayState::default();
        update_active_tab_locked(&mut state, Tab::Agent);
        assert_eq!(state.active_tab, Tab::Agent);

        update_active_tab_locked(&mut state, Tab::Drawer);
        assert_eq!(state.active_tab, Tab::Drawer);
    }

    #[test]
    #[serial]
    fn handoff_transcript_to_chat_adds_user_message_without_callback() {
        {
            let mut cb = SEND_CALLBACK.lock().unwrap_or_else(|e| e.into_inner());
            *cb = None;
        }
        {
            let mut state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
            *state = VoiceChatOverlayState::default();
        }

        handoff_transcript_to_chat_impl("transcript payload");

        let state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
        assert_eq!(state.messages.len(), 1);
        assert_eq!(state.messages[0].role, ChatRole::User);
        assert_eq!(state.messages[0].text, "transcript payload");
        assert!(
            !state.is_sending,
            "without callback, handoff must not stay in sending state"
        );
    }

    #[test]
    #[serial]
    fn handoff_transcript_to_chat_invokes_callback() {
        let call_count = Arc::new(AtomicUsize::new(0));
        let observed = Arc::new(Mutex::new(String::new()));
        {
            let count = Arc::clone(&call_count);
            let observed = Arc::clone(&observed);
            let mut cb = SEND_CALLBACK.lock().unwrap_or_else(|e| e.into_inner());
            *cb = Some(Arc::new(move |text: String| {
                count.fetch_add(1, Ordering::SeqCst);
                let mut guard = observed.lock().unwrap_or_else(|e| e.into_inner());
                *guard = text;
            }));
        }
        {
            let mut state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
            *state = VoiceChatOverlayState::default();
        }

        handoff_transcript_to_chat_impl("augment this");

        assert_eq!(call_count.load(Ordering::SeqCst), 1);
        let payload = observed.lock().unwrap_or_else(|e| e.into_inner()).clone();
        assert_eq!(payload, "augment this");

        let state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
        assert_eq!(state.messages.len(), 1);
        assert!(state.is_sending);

        let mut cb = SEND_CALLBACK.lock().unwrap_or_else(|e| e.into_inner());
        *cb = None;
    }

    #[test]
    #[serial]
    fn dispatch_voice_chat_send_returns_false_without_callback() {
        let mut cb = SEND_CALLBACK.lock().unwrap_or_else(|e| e.into_inner());
        *cb = None;
        drop(cb);

        assert!(!dispatch_voice_chat_send("payload"));
        assert!(!dispatch_voice_chat_send("   "));
    }

    #[test]
    #[serial]
    fn dispatch_voice_chat_send_invokes_callback() {
        let call_count = Arc::new(AtomicUsize::new(0));
        let observed = Arc::new(Mutex::new(String::new()));
        {
            let count = Arc::clone(&call_count);
            let observed = Arc::clone(&observed);
            let mut cb = SEND_CALLBACK.lock().unwrap_or_else(|e| e.into_inner());
            *cb = Some(Arc::new(move |text: String| {
                count.fetch_add(1, Ordering::SeqCst);
                let mut guard = observed.lock().unwrap_or_else(|e| e.into_inner());
                *guard = text;
            }));
        }

        assert!(dispatch_voice_chat_send("runtime payload"));
        assert_eq!(call_count.load(Ordering::SeqCst), 1);
        let payload = observed.lock().unwrap_or_else(|e| e.into_inner()).clone();
        assert_eq!(payload, "runtime payload");

        let mut cb = SEND_CALLBACK.lock().unwrap_or_else(|e| e.into_inner());
        *cb = None;
    }
}

fn create_drawer_card(
    entry: &DrawerEntry,
    index: usize,
    handler: Option<usize>,
    query: &str,
) -> Id {
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
        let subtitle = drawer_entry_subtitle(entry);
        let preview = entry.preview.clone();
        let card = create_card_view(frame, &title, &subtitle, &preview);
        // Highlight matching query text in the preview field (last NSTextField subview).
        if !query.is_empty() {
            let subviews: Id = msg_send![card, subviews];
            let count: usize = msg_send![subviews, count];
            // The preview field is typically the 3rd text field added (index 2).
            // Walk subviews in reverse to find it (last NSTextField before action buttons).
            for i in (0..count).rev() {
                let subview: Id = msg_send![subviews, objectAtIndex: i];
                let ns_text_field = Class::get("NSTextField").unwrap();
                let is_text_field: bool = msg_send![subview, isKindOfClass: ns_text_field];
                if is_text_field {
                    apply_search_highlight(subview, &preview, query);
                    break;
                }
            }
        }

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
                    &core_graphics::geometry::CGSize::new(64.0, 20.0),
                ),
                title,
                crate::ui_helpers::button_style::ROUNDED,
            );
            let supports_control_size: bool =
                msg_send![button, respondsToSelector: sel!(setControlSize:)];
            if supports_control_size {
                let _: () = msg_send![button, setControlSize: 1_isize]; // NSSmallControlSize
            }
            if let Some(handler) = handler {
                crate::ui_helpers::button_set_action(button, handler as Id, button_actions[idx]);
            }
            let _: () = msg_send![button, setTag: index as isize];
            let _: () = msg_send![actions_container, addSubview: button];
        }

        let favorite = crate::ui_helpers::create_button(
            core_graphics::geometry::CGRect::new(
                &CGPoint::new(230.0, 0.0),
                &core_graphics::geometry::CGSize::new(28.0, 20.0),
            ),
            "",
            crate::ui_helpers::button_style::INLINE,
        );
        let fav_symbol = if entry.is_favorite {
            "heart.fill"
        } else {
            "heart"
        };
        let _ = set_button_symbol(favorite, fav_symbol);
        crate::ui_helpers::style_toolbar_icon_button(favorite);
        let supports_control_size: bool =
            msg_send![favorite, respondsToSelector: sel!(setControlSize:)];
        if supports_control_size {
            let _: () = msg_send![favorite, setControlSize: 1_isize];
        }
        if let Some(handler) = handler {
            crate::ui_helpers::button_set_action(favorite, handler as Id, sel!(onCardFavorite:));
        }
        set_tooltip(favorite, "Favorite");
        let _: () = msg_send![favorite, setTag: index as isize];
        let _: () = msg_send![actions_container, addSubview: favorite];

        let _: () = msg_send![card, addSubview: actions_container];
        card
    }
}

/// NSRange for Objective-C attributed string APIs.
#[repr(C)]
#[derive(Copy, Clone)]
struct NSRange {
    location: usize,
    length: usize,
}

/// Apply search-term highlighting to a text field by bolding matching ranges.
///
/// Uses `char_indices()` to safely iterate over Unicode characters, then maps
/// character offsets to UTF-16 code unit counts for `NSRange` (Cocoa convention).
unsafe fn apply_search_highlight(field: Id, text: &str, query: &str) {
    let ns_mut_attr = Class::get("NSMutableAttributedString").unwrap();
    let ns_font_cls = Class::get("NSFont").unwrap();
    let text_ns = ns_string(text);
    let attr_str: Id = msg_send![ns_mut_attr, alloc];
    let attr_str: Id = msg_send![attr_str, initWithString: text_ns];
    let bold_font: Id = msg_send![ns_font_cls, boldSystemFontOfSize: ui_tokens::BODY_FONT_SIZE];
    let font_key = ns_string("NSFont");
    // Build char-level lowercase for safe matching (no byte-index slicing).
    let text_chars: Vec<char> = text.chars().collect();
    let text_lower: Vec<char> = text_chars
        .iter()
        .map(|c| c.to_lowercase().next().unwrap_or(*c))
        .collect();
    let query_lower: Vec<char> = query
        .chars()
        .map(|c| c.to_lowercase().next().unwrap_or(c))
        .collect();
    if query_lower.is_empty() {
        // Always set the plain attributed string to clear stale highlights.
        let _: () = msg_send![field, setAttributedStringValue: attr_str];
        return;
    }
    // Build byte→utf16 offset map at char boundaries for NSRange conversion.
    let mut char_to_utf16: Vec<usize> = Vec::with_capacity(text_chars.len() + 1);
    let mut utf16_pos: usize = 0;
    for ch in &text_chars {
        char_to_utf16.push(utf16_pos);
        utf16_pos += ch.len_utf16();
    }
    char_to_utf16.push(utf16_pos); // sentinel for end
    // Slide through char-level arrays to find matches.
    let mut i = 0;
    while i + query_lower.len() <= text_lower.len() {
        if text_lower[i..i + query_lower.len()] == query_lower[..] {
            let range = NSRange {
                location: char_to_utf16[i],
                length: char_to_utf16[i + query_lower.len()] - char_to_utf16[i],
            };
            let _: () = msg_send![attr_str, addAttribute: font_key value: bold_font range: range];
            let highlight = ui_colors::search_highlight_bg();
            let bg_key = ns_string("NSBackgroundColor");
            let _: () = msg_send![attr_str, addAttribute: bg_key value: highlight range: range];
            i += query_lower.len();
        } else {
            i += 1;
        }
    }
    let _: () = msg_send![field, setAttributedStringValue: attr_str];
}
fn entry_type_label(entry: &DrawerEntry) -> &'static str {
    if is_drawer_unavailable_placeholder(entry) {
        return "Warning";
    }
    match entry.source {
        DrawerEntrySource::Thread { .. } => "ThreadStore",
        DrawerEntrySource::LegacyFile => {
            if entry.is_ai_formatted {
                "Legacy AI"
            } else {
                "Legacy Raw"
            }
        }
    }
}

fn drawer_entry_source_label(entry: &DrawerEntry) -> String {
    if is_drawer_unavailable_placeholder(entry) {
        return "ThreadStore".to_string();
    }
    match entry.source {
        DrawerEntrySource::Thread { .. } => {
            if entry.path.exists() {
                "ThreadStore".to_string()
            } else {
                "ThreadStore (index-only)".to_string()
            }
        }
        DrawerEntrySource::LegacyFile => "Legacy transcript file".to_string(),
    }
}

fn drawer_entry_subtitle(entry: &DrawerEntry) -> String {
    if is_drawer_unavailable_placeholder(entry) {
        return "Shift/Cmd • ThreadStore • unavailable".to_string();
    }
    let source_label = drawer_entry_source_label(entry);
    match &entry.source {
        DrawerEntrySource::Thread { id } => {
            format!(
                "{} • {} • thread:{id}",
                mode_label(entry.mode),
                source_label
            )
        }
        DrawerEntrySource::LegacyFile => {
            format!(
                "{} • {} • {}",
                mode_label(entry.mode),
                source_label,
                entry.path.display()
            )
        }
    }
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
    load_drawer_entries_with_query("")
}

fn load_drawer_entries_with_query(query: &str) -> Vec<DrawerEntry> {
    let favorites = load_favorites_from_disk();
    let mut entries = load_thread_drawer_entries(&favorites);
    entries.sort_by_key(|b| std::cmp::Reverse(b.timestamp));

    let query_lower = query.trim().to_ascii_lowercase();
    if !query_lower.is_empty() {
        entries.retain(|entry| drawer_entry_matches_query(entry, &query_lower));
    }

    entries
}

fn thread_history_unavailable_drawer_entry() -> DrawerEntry {
    DrawerEntry {
        source: DrawerEntrySource::LegacyFile,
        path: PathBuf::from(""),
        timestamp: SystemTime::now(),
        mode: TranscriptionMode::Assistive,
        preview: "Thread history unavailable — storage error".to_string(),
        search_corpus: "thread history unavailable storage error".to_string(),
        is_ai_formatted: false,
        is_favorite: false,
    }
}

fn is_drawer_unavailable_placeholder(entry: &DrawerEntry) -> bool {
    matches!(entry.source, DrawerEntrySource::LegacyFile) && entry.path.as_os_str().is_empty()
}

fn load_thread_drawer_entries(favorites: &HashSet<String>) -> Vec<DrawerEntry> {
    let Ok(store) = ThreadStore::new() else {
        warn!("Drawer: failed to open ThreadStore; drawer entries unavailable");
        return vec![thread_history_unavailable_drawer_entry()];
    };
    let Ok(index) = ThreadIndex::load_or_create(store.threads_dir()) else {
        warn!("Drawer: failed to load ThreadIndex; drawer entries unavailable");
        return vec![thread_history_unavailable_drawer_entry()];
    };

    index
        .list(None)
        .into_iter()
        .map(|summary| {
            let id = summary.id.clone();
            let source = DrawerEntrySource::Thread { id: id.clone() };
            let favorite_key = format!("thread:{id}");
            let mut preview = summary
                .latest_note
                .as_deref()
                .or(summary.latest_message.as_deref())
                .or(summary.summary.as_deref())
                .unwrap_or(summary.title.as_str())
                .to_string();
            let mut search_corpus = summary.search_text.clone();
            if (search_corpus.trim().is_empty() || preview.trim().is_empty())
                && let Ok(thread) = store.load_thread(&id)
            {
                if preview.trim().is_empty() {
                    preview = thread_preview_for_drawer(&thread);
                }
                if search_corpus.trim().is_empty() {
                    search_corpus = thread_search_corpus_for_drawer(&thread);
                }
            }
            preview = normalize_preview(&preview, 120);
            let path = store
                .thread_file_path(&id)
                .unwrap_or_else(|_| PathBuf::from(format!("thread_{id}.json")));
            let timestamp = system_time_from_unix_millis(summary.updated_at.timestamp_millis());
            let mode = transcription_mode_from_thread_mode(&summary.mode);
            let mode_label = mode_label(mode);
            if search_corpus.trim().is_empty() {
                search_corpus = format!(
                    "{} {} {} {}",
                    summary.title,
                    summary.mode,
                    summary.summary.as_deref().unwrap_or_default(),
                    preview
                );
            }
            search_corpus = format!(
                "threadstore source:thread {} thread:{} {}",
                mode_label, id, search_corpus
            )
            .to_ascii_lowercase();

            DrawerEntry {
                source,
                path,
                timestamp,
                mode,
                preview,
                search_corpus,
                is_ai_formatted: true,
                is_favorite: summary.is_favorite || favorites.contains(&favorite_key),
            }
        })
        .collect()
}

fn system_time_from_unix_millis(timestamp_millis: i64) -> SystemTime {
    if timestamp_millis <= 0 {
        return SystemTime::now();
    }
    UNIX_EPOCH + Duration::from_millis(timestamp_millis as u64)
}

fn transcription_mode_from_thread_mode(mode: &str) -> TranscriptionMode {
    if mode.eq_ignore_ascii_case("conversation") || mode.eq_ignore_ascii_case("moshi") {
        TranscriptionMode::Conversation
    } else if mode.eq_ignore_ascii_case("assistive") || mode.eq_ignore_ascii_case("chat") {
        TranscriptionMode::Assistive
    } else if mode.eq_ignore_ascii_case("hold") || mode.eq_ignore_ascii_case("raw") {
        TranscriptionMode::Hold
    } else {
        TranscriptionMode::Toggle
    }
}

fn normalize_preview(text: &str, max_chars: usize) -> String {
    text.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .take(max_chars)
        .collect::<String>()
}

fn thread_preview_for_drawer(thread: &Thread) -> String {
    if let Some(summary) = &thread.summary
        && !summary.trim().is_empty()
    {
        return normalize_preview(summary, 120);
    }
    if let Some(note) = thread
        .notes
        .iter()
        .rev()
        .find(|note| !note.text.trim().is_empty())
    {
        return normalize_preview(&note.text, 120);
    }
    for message in thread.messages.iter().rev() {
        let text = thread_message_text_for_copy(message);
        if !text.trim().is_empty() {
            return normalize_preview(&text, 120);
        }
    }
    normalize_preview(&thread.title, 120)
}

fn thread_search_corpus_for_drawer(thread: &Thread) -> String {
    let mut pieces = vec![thread.title.clone(), thread.mode.clone()];
    if let Some(summary) = &thread.summary {
        pieces.push(summary.clone());
    }
    for note in &thread.notes {
        pieces.push(note.text.clone());
    }
    for message in &thread.messages {
        pieces.push(thread_message_text_for_copy(message));
    }
    pieces
        .join(" ")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_ascii_lowercase()
}

fn drawer_entry_favorite_key(entry: &DrawerEntry) -> String {
    match &entry.source {
        DrawerEntrySource::Thread { id } => format!("thread:{id}"),
        DrawerEntrySource::LegacyFile => entry.path.to_string_lossy().to_string(),
    }
}

fn thread_markdown_for_copy(thread: &Thread) -> String {
    let mut out = String::new();
    let title = thread.title.trim();
    let title = if title.is_empty() {
        "Untitled Thread"
    } else {
        title
    };
    out.push_str("# ");
    out.push_str(title);
    out.push_str("\n\n");

    if let Some(summary) = &thread.summary
        && !summary.trim().is_empty()
    {
        out.push_str("## Summary\n");
        out.push_str(summary.trim());
        out.push_str("\n\n");
    }

    if !thread.notes.is_empty() {
        out.push_str("## Notes\n");
        for note in &thread.notes {
            out.push_str("- ");
            out.push_str(note.text.trim());
            if let Some(anchor) = note.anchored_to_message {
                out.push_str(&format!(" (anchor: #{anchor})"));
            }
            out.push('\n');
        }
        out.push('\n');
    }

    if !thread.messages.is_empty() {
        out.push_str("## Messages\n");
        for message in &thread.messages {
            out.push_str("### ");
            out.push_str(&message.role.to_ascii_uppercase());
            out.push('\n');
            out.push_str(thread_message_text_for_copy(message).trim());
            out.push_str("\n\n");
        }
    }

    out.trim_end().to_string()
}

fn thread_message_text_for_copy(message: &codescribe_core::agent::ThreadMessage) -> String {
    let mut chunks = Vec::new();
    for value in &message.content {
        collect_copy_text(value, &mut chunks);
    }
    let text = chunks.join(" ");
    if text.trim().is_empty() {
        "(non-text content)".to_string()
    } else {
        text
    }
}

fn collect_copy_text(value: &serde_json::Value, out: &mut Vec<String>) {
    match value {
        serde_json::Value::String(text) if !text.trim().is_empty() => {
            out.push(text.to_string());
        }
        serde_json::Value::Array(items) => {
            if items.iter().all(serde_json::Value::is_number) {
                return;
            }
            for item in items {
                collect_copy_text(item, out);
            }
        }
        serde_json::Value::Object(map) => {
            if let Some(text) = map.get("text").and_then(serde_json::Value::as_str)
                && !text.trim().is_empty()
            {
                out.push(text.to_string());
            }
            if let Some(content) = map.get("content") {
                collect_copy_text(content, out);
            }
            if let Some(input) = map.get("input") {
                collect_copy_text(input, out);
            }
            for (key, nested) in map {
                if matches!(key.as_str(), "text" | "content" | "input" | "data") {
                    continue;
                }
                collect_copy_text(nested, out);
            }
        }
        _ => {}
    }
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

fn load_favorites_from_disk() -> HashSet<String> {
    let path = favorites_path();
    let Ok(data) = std::fs::read_to_string(&path) else {
        return HashSet::new();
    };
    let Ok(file) = serde_json::from_str::<FavoritesFile>(&data) else {
        return HashSet::new();
    };
    file.paths.into_iter().collect()
}

fn save_favorites_to_disk(favorites: &HashSet<String>) {
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
