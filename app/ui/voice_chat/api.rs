//! Public API and internal helpers for voice chat overlay
//!
//! Contains all the public functions for controlling the overlay and
//! internal helper functions for state updates.

use core_graphics::geometry::{CGPoint, CGRect, CGSize};
use dispatch::Queue;
use objc::runtime::{Class, Object};
use objc::{msg_send, sel, sel_impl};
use std::path::PathBuf;
use std::time::{Duration, Instant, SystemTime};
use tracing::{debug, info, warn};

use chrono::{DateTime, Local};

use crate::ui::shared::status::{UiStatus, status_from_detail};
use crate::ui_helpers::{
    BubbleConfig, BubbleRole, LabelConfig, add_subview, button_set_action, button_style,
    color_label, color_rgba, color_secondary_label, create_bubble_view, create_button,
    create_card_view, create_label, get_text_field_string, get_text_view_string,
    layout_region_frame_for_view, list_draft_files, ns_string, open_file_in_editor,
    resize_bubble_container_for_text, set_button_symbol, set_hidden, set_text_field_string,
    set_text_view_string, set_tooltip, stack_view_add, stack_view_clear, ui_colors, ui_tokens,
    update_bubble_text, window_set_alpha, window_show,
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

/// Add a user message to the chat
pub fn add_voice_chat_user_message(text: &str) {
    let text_owned = text.to_string();
    Queue::main().exec_async(move || {
        let mut state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
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

/// Switch to Transcription tab programmatically
pub fn show_transcription_tab() {
    Queue::main().exec_async(|| {
        update_active_tab_impl(Tab::Transcription);
    });
}

/// Switch to Settings tab programmatically
pub fn show_settings_tab() {
    Queue::main().exec_async(|| {
        crate::show_bootstrap_overlay();
    });
}

/// Request Settings tab to be shown the next time the overlay is created.
/// This is used when routing tray "Settings" to the overlay before it exists.
pub fn request_settings_tab_on_open() {
    crate::show_bootstrap_overlay();
}

/// Append a delta (streaming token) to the transcription preview.
pub fn append_transcription_delta(delta: &str) {
    let delta_owned = delta.to_string();
    Queue::main().exec_async(move || {
        append_transcription_delta_impl(&delta_owned);
    });
}

fn append_transcription_delta_impl(delta: &str) {
    let mut state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
    codescribe_core::pipeline::contracts::TranscriptDelta::from_raw(delta)
        .apply(&mut state.transcription_text);
    if let Some(text_view) = state.transcription_text_view {
        unsafe { set_text_view_string(text_view as Id, &state.transcription_text) };
    }
    update_transcription_placeholder(&mut state);
}

/// Set the full transcription text (used for final-pass replacement).
pub fn set_transcription_text(text: &str) {
    let text_owned = text.to_string();
    Queue::main().exec_async(move || {
        set_transcription_text_impl(&text_owned);
    });
}

fn set_transcription_text_impl(text: &str) {
    let mut state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
    state.transcription_text = text.to_string();
    if let Some(text_view) = state.transcription_text_view {
        unsafe { set_text_view_string(text_view as Id, &state.transcription_text) };
    }
    update_transcription_placeholder(&mut state);
}

/// Clear the transcription preview text.
pub fn clear_transcription_text() {
    Queue::main().exec_async(|| {
        clear_transcription_text_impl();
    });
}

fn clear_transcription_text_impl() {
    let mut state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
    state.transcription_text.clear();
    if let Some(text_view) = state.transcription_text_view {
        unsafe { set_text_view_string(text_view as Id, "") };
    }
    update_transcription_placeholder(&mut state);
}

fn is_transcription_empty(state: &VoiceChatOverlayState) -> bool {
    state.transcription_text.trim().is_empty()
}

fn update_transcription_placeholder(state: &mut VoiceChatOverlayState) {
    let Some(view_ptr) = state.transcription_placeholder else {
        return;
    };
    let should_show = state.active_tab == Tab::Transcription && is_transcription_empty(state);
    unsafe { set_hidden(view_ptr as Id, !should_show) };
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
    if tab == Tab::Settings {
        crate::show_bootstrap_overlay();
        return;
    }
    let mut state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
    update_active_tab_locked(&mut state, tab);
}

fn update_active_tab_locked(state: &mut VoiceChatOverlayState, tab: Tab) {
    unsafe {
        if tab == Tab::Settings {
            return;
        }
        state.active_tab = tab;

        if let Some(button) = state.tab_drawer_button {
            crate::ui_helpers::set_tab_button_active(button as Id, tab == Tab::Drawer);
        }
        if let Some(button) = state.tab_transcription_button {
            crate::ui_helpers::set_tab_button_active(button as Id, tab == Tab::Transcription);
        }
        if let Some(button) = state.tab_agent_button {
            crate::ui_helpers::set_tab_button_active(button as Id, tab == Tab::Agent);
        }
        if let Some(button) = state.tab_settings_button {
            crate::ui_helpers::set_tab_button_active(button as Id, false);
        }

        let show_drawer = tab == Tab::Drawer;
        let show_transcription = tab == Tab::Transcription;
        let show_agent = tab == Tab::Agent;

        if let Some(sidebar_item) = state.split_sidebar_item {
            let item = sidebar_item as Id;
            let responds: bool = msg_send![item, respondsToSelector: sel!(setCollapsed:)];
            if responds {
                let _: () = msg_send![item, setCollapsed: show_agent];
            }
        }
        if let Some(content_item) = state.split_content_item {
            let item = content_item as Id;
            let responds: bool = msg_send![item, respondsToSelector: sel!(setCollapsed:)];
            if responds {
                let _: () = msg_send![item, setCollapsed: !show_agent];
            }
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
        if let Some(trans_view) = state.transcription_scroll_view {
            crate::ui_helpers::set_hidden(trans_view as Id, !show_transcription);
        }
        if let Some(edge) = state.transcription_edge_effect {
            crate::ui_helpers::set_hidden(edge as Id, !show_transcription);
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

        if show_agent {
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

        update_transcription_placeholder(state);
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
    reflow_footer_controls_locked(&mut state);
    resize_agent_input_locked(&mut state);
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

        let header_height = ui_tokens::HEADER_HEIGHT;
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
    state.status_text = status.to_string();
    let next_kind = status_from_detail(status);
    state.status_kind = next_kind;
    apply_status_pill(&state);
    let _ = crate::tray::update_tray_status(next_kind.to_tray());
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
            let bg = color_rgba(palette.bg.0, palette.bg.1, palette.bg.2, palette.bg.3);
            let cg: Id = msg_send![bg, CGColor];
            let _: () = msg_send![layer, setBackgroundColor: cg];
        }

        if let Some(label_ptr) = state.status_pill_label {
            let label = label_ptr as Id;
            let _: () = msg_send![label, setStringValue: ns_string(state.status_kind.label())];
            let text_color = color_rgba(
                palette.text.0,
                palette.text.1,
                palette.text.2,
                palette.text.3,
            );
            let _: () = msg_send![label, setTextColor: text_color];
        }

        if let Some(dot_ptr) = state.status_pill_dot {
            let dot = dot_ptr as Id;
            let dot_layer: Id = msg_send![dot, layer];
            if !dot_layer.is_null() {
                let dot_color =
                    color_rgba(palette.dot.0, palette.dot.1, palette.dot.2, palette.dot.3);
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
                            msg_send![Class::get("NSNumber").unwrap(), numberWithFloat: 1.0f32];
                        let to_val: Id =
                            msg_send![Class::get("NSNumber").unwrap(), numberWithFloat: 0.3f32];
                        let _: () = msg_send![anim, setFromValue: from_val];
                        let _: () = msg_send![anim, setToValue: to_val];
                        let _: () = msg_send![anim, setDuration: 0.8f64];
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

    width
        .min(ui_tokens::BUBBLE_MAX_WIDTH)
        .clamp(240.0, ui_tokens::BUBBLE_MAX_WIDTH)
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
        update_bubble_text(label, &message.text, message.is_streaming);
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
    let idx = last_message_index(&state, ChatRole::User).unwrap_or_else(|| {
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
    });
    if let Some(msg) = state.messages.get_mut(idx) {
        msg.text = text.to_string();
        msg.is_streaming = false;
        msg.is_error = false;
    }
    update_chat_view_with_state(&mut state, true);
}

fn finalize_user_message_state_only_impl() {
    let mut state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(last) = state
        .messages
        .iter_mut()
        .rev()
        .find(|msg| msg.role == ChatRole::User)
    {
        last.is_streaming = false;
        last.is_error = false;
    } else {
        return;
    }
    update_chat_view_with_state(&mut state, true);
}

fn finalize_assistant_message_impl(text: &str, is_error: bool) {
    let mut state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
    ensure_agent_tab_visible(&mut state);
    let idx = last_message_index(&state, ChatRole::Assistant).unwrap_or_else(|| {
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
    });
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
    if let Some(last) = state
        .messages
        .iter_mut()
        .rev()
        .find(|msg| msg.role == ChatRole::Assistant)
    {
        last.is_streaming = false;
        last.is_error = is_error;
    } else {
        return;
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
        update_active_tab_locked(state, Tab::Agent);
    }
}

pub(super) fn clear_voice_chat_text_impl() {
    let btn_ptr = {
        let mut state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
        state.messages.clear();
        state.manual_draft.clear();
        state.is_sending = false;
        state.attached_files.clear();
        state.attached_files_last_sent = None;
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

        let attachments_to_send = attachment_should_include_locked(&state);
        if let Some((fingerprint, _paths, summary)) = attachments_to_send.as_ref() {
            let mode = message_mode_label(&state);
            state.messages.push(ChatMessage {
                role: ChatRole::System,
                text: format!("Attachments (sent once): {}", summary),
                is_streaming: false,
                is_error: false,
                timestamp: SystemTime::now(),
                mode: Some(mode),
            });
            state.attached_files_last_sent = Some(*fingerprint);
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
            let mode = message_mode_label(&state);
            state.messages.push(ChatMessage {
                role: ChatRole::System,
                text: format!("Attachments (sent once): {}", summary),
                is_streaming: false,
                is_error: false,
                timestamp: SystemTime::now(),
                mode: Some(mode),
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

fn last_message_index(state: &VoiceChatOverlayState, role: ChatRole) -> Option<usize> {
    state.messages.iter().rposition(|msg| msg.role == role)
}

fn get_or_create_streaming_message_index(
    state: &mut VoiceChatOverlayState,
    role: ChatRole,
) -> usize {
    if let Some(idx) = state
        .messages
        .iter()
        .rposition(|msg| msg.role == role && msg.is_streaming)
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
    state.messages.len() - 1
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

        // Empty state CTA when no messages exist yet.
        if state.messages.is_empty() {
            let empty_label = create_label(LabelConfig {
                frame: CGRect::new(&CGPoint::new(0.0, 0.0), &CGSize::new(max_width, 60.0)),
                text: "Start a conversation\nPress hotkey to record \u{2022} Type to send"
                    .to_string(),
                font_size: 13.0,
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
            let title = if state.is_sending { "…" } else { ">" };
            let title = ns_string(title);
            let _: () = msg_send![btn, setTitle: title];
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
        let has_symbol = crate::ui_helpers::set_button_symbol(btn, "doc.badge.plus");
        let title = if count == 0 {
            if has_symbol {
                String::new()
            } else {
                "Attach".to_string()
            }
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
        if height_same && width_same {
            return;
        }

        // Resize input bar (anchored to bottom).
        let new_bar_frame = CGRect::new(
            &CGPoint::new(pad, footer_inset),
            &CGSize::new(bar_width, desired_h),
        );
        let _: () = msg_send![input_bar, setFrame: new_bar_frame];

        // Resize the scrollable text view inside the bar.
        let text_area_frame = CGRect::new(
            &CGPoint::new(12.0, 10.0),
            &CGSize::new((bar_width - 140.0).max(120.0), (desired_h - 20.0).max(24.0)),
        );
        let _: () = msg_send![input_scroll, setFrame: text_area_frame];

        // Recenter buttons vertically.
        let send_y = ((desired_h - 32.0) / 2.0).max(8.0);
        let attach_frame = CGRect::new(
            &CGPoint::new((bar_width - 120.0).max(0.0), send_y),
            &CGSize::new(36.0, 32.0),
        );
        let _: () = msg_send![attach_btn, setFrame: attach_frame];
        let send_frame = CGRect::new(
            &CGPoint::new((bar_width - 76.0).max(0.0), send_y),
            &CGSize::new(36.0, 32.0),
        );
        let _: () = msg_send![send_btn, setFrame: send_frame];

        // Resize Agent scroll view so it doesn't overlap with input.
        if let Some(agent_scroll_ptr) = state.agent_scroll_view {
            let agent_scroll = agent_scroll_ptr as Id;
            let bottom = footer_inset + desired_h + input_gap;
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
    state.tab_transcription_button = None;
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
    state.attached_files.clear();
    state.attached_files_last_sent = None;
    state.transcription_scroll_view = None;
    state.transcription_text_view = None;
    state.transcription_placeholder = None;
    state.transcription_edge_effect = None;
    state.active_tab = Tab::Drawer;
    state.pending_tab = None;
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
    let (path, window_usize) = {
        let state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
        let path = state.drawer_entries.get(index).map(|e| e.path.clone());
        (path, state.window)
    };

    let Some(path) = path else {
        return;
    };

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
        if !filter.is_empty() {
            let hay = entry.preview.to_lowercase();
            if !hay.contains(&filter) {
                continue;
            }
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
            let _: () = msg_send![layer, setCornerRadius: ui_tokens::CORNER_RADIUS_MD];
            let border = ui_colors::separator();
            let border: Id = msg_send![border, colorWithAlphaComponent: 0.3f64];
            let cg_border: Id = msg_send![border, CGColor];
            let _: () = msg_send![layer, setBorderColor: cg_border];
            let _: () = msg_send![layer, setBorderWidth: 1.0f64];
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
            text: "Or show the overlay to begin.".to_string(),
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
            "Show overlay",
            button_style::ROUNDED,
        );

        if let Some(handler_ptr) = handler {
            let handler_id = handler_ptr as Id;
            button_set_action(start_button, handler_id, sel!(onStartRecording:));
            button_set_action(overlay_button, handler_id, sel!(onShowOverlay:));
        }

        set_tooltip(
            start_button,
            "Hotkey: hold Ctrl (or your configured hold keys)",
        );
        set_tooltip(overlay_button, "Bring CodeScribe overlay to front");
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

    #[test]
    fn filtered_drawer_entries_respects_query_and_favorites() {
        let mut state = VoiceChatOverlayState {
            drawer_entries: vec![
                DrawerEntry {
                    path: PathBuf::from("a.txt"),
                    timestamp: SystemTime::now(),
                    mode: TranscriptionMode::Hold,
                    preview: "hello world".to_string(),
                    is_ai_formatted: false,
                    is_favorite: false,
                },
                DrawerEntry {
                    path: PathBuf::from("b.txt"),
                    timestamp: SystemTime::now(),
                    mode: TranscriptionMode::Assistive,
                    preview: "favorite note".to_string(),
                    is_ai_formatted: false,
                    is_favorite: true,
                },
            ],
            ..Default::default()
        };

        assert_eq!(filtered_drawer_entries(&state, "hello").len(), 1);
        state.drawer_favorites_only = true;
        assert_eq!(filtered_drawer_entries(&state, "").len(), 1);
    }

    #[test]
    fn transcription_empty_detection() {
        let mut state = VoiceChatOverlayState::default();
        assert!(is_transcription_empty(&state));
        state.transcription_text = "hi".to_string();
        assert!(!is_transcription_empty(&state));
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
    fn update_active_tab_handles_settings_without_views() {
        let mut state = VoiceChatOverlayState::default();
        update_active_tab_locked(&mut state, Tab::Settings);
        assert_eq!(state.active_tab, Tab::Drawer);

        update_active_tab_locked(&mut state, Tab::Drawer);
        assert_eq!(state.active_tab, Tab::Drawer);
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
        let subtitle = format!("{} • {}", mode_label(entry.mode), entry.path.display());
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
                let responds: bool =
                    msg_send![subview, respondsToSelector: sel!(attributedStringValue)];
                if responds {
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

/// NSRange for Objective-C attributed string APIs.
#[repr(C)]
#[derive(Copy, Clone)]
struct NSRange {
    location: usize,
    length: usize,
}

/// Apply search-term highlighting to a text field by bolding matching ranges.
unsafe fn apply_search_highlight(field: Id, text: &str, query: &str) {
    let ns_mut_attr = Class::get("NSMutableAttributedString").unwrap();
    let ns_font_cls = Class::get("NSFont").unwrap();
    let text_ns = ns_string(text);
    let attr_str: Id = msg_send![ns_mut_attr, alloc];
    let attr_str: Id = msg_send![attr_str, initWithString: text_ns];
    let bold_font: Id = msg_send![ns_font_cls, boldSystemFontOfSize: ui_tokens::BODY_FONT_SIZE];
    let font_key = ns_string("NSFont");
    let query_lower = query.to_lowercase();
    let text_lower = text.to_lowercase();
    let mut start = 0;
    while let Some(pos) = text_lower[start..].find(&query_lower) {
        let abs_pos = start + pos;
        let range = NSRange {
            location: abs_pos,
            length: query_lower.len(),
        };
        let _: () = msg_send![attr_str, addAttribute: font_key value: bold_font range: range];
        // Also set highlight color for visibility.
        let highlight = color_rgba(255.0, 210.0, 0.0, 0.3);
        let bg_key = ns_string("NSBackgroundColor");
        let _: () = msg_send![attr_str, addAttribute: bg_key value: highlight range: range];
        start = abs_pos + query_lower.len();
    }
    let _: () = msg_send![field, setAttributedStringValue: attr_str];
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
