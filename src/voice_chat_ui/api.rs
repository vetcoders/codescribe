//! Public API and internal helpers for voice chat overlay
//!
//! Contains all the public functions for controlling the overlay and
//! internal helper functions for state updates.

use core_graphics::geometry::CGPoint;
use dispatch::Queue;
use objc::runtime::{Class, Object};
use objc::{msg_send, sel, sel_impl};
use tracing::{debug, info};

use crate::ui_helpers::{
    BubbleConfig, BubbleRole, create_bubble_view, list_draft_files, ns_string, stack_view_add,
    stack_view_clear, update_bubble_text,
};

use super::state::{ChatMessage, ChatRole, OVERLAY_STATE, SEND_CALLBACK, VoiceChatOverlayState};

// Type alias for Objective-C object pointers
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

/// Append a delta (streaming token) to the voice draft
pub fn append_voice_chat_delta(delta: &str) {
    let delta_owned = delta.to_string();
    Queue::main().exec_async(move || {
        append_voice_chat_draft_impl(&delta_owned);
    });
}

/// Finalize voice draft: save to file and clear buffer
/// Called when VAD stops or recording finishes
pub fn finalize_voice_draft() -> Option<std::path::PathBuf> {
    Queue::main().exec_sync(finalize_voice_draft_impl)
}

/// Get the current voice draft text (for reading without clearing)
pub fn get_voice_draft() -> String {
    let state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
    state.voice_draft.clone()
}

/// Clear voice draft without saving (e.g., on cancel)
pub fn clear_voice_draft() {
    Queue::main().exec_async(|| {
        let mut state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
        state.voice_draft.clear();
        state.is_voice_active = false;
        update_voice_draft_view_with_state(&mut state);
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

/// Set the current voice draft text (streaming from Whisper)
pub fn set_voice_chat_draft_text(text: &str) {
    let text_owned = text.to_string();
    Queue::main().exec_async(move || {
        let mut state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
        state.voice_draft = text_owned;
        state.is_voice_active = true;
        update_voice_draft_view_with_state(&mut state);
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

/// Get the current voice draft text from the overlay (for auto-send)
pub fn get_accumulated_text() -> String {
    let state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
    state.voice_draft.clone()
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

// ═══════════════════════════════════════════════════════════
// Internal Implementation Functions
// ═══════════════════════════════════════════════════════════

fn update_voice_chat_status_impl(status: &str) {
    unsafe {
        let state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(status_field_ptr) = state.status_field {
            let status_field = status_field_ptr as Id;
            let ns_string_class = Class::get("NSString").unwrap();

            // Create null-terminated C string
            let mut c_str = status.as_bytes().to_vec();
            c_str.push(0);

            let ns_str: Id = msg_send![ns_string_class, stringWithUTF8String: c_str.as_ptr()];
            let _: () = msg_send![status_field, setStringValue: ns_str];
        }
    }
}

fn append_voice_chat_draft_impl(delta: &str) {
    let mut state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
    // Voice streaming goes to voice_draft (right panel / sidecar)
    state.voice_draft.push_str(delta);
    state.is_voice_active = true;
    update_voice_draft_view_with_state(&mut state);
    // Note: We don't update manual input field here - they are separate
}

fn finalize_voice_draft_impl() -> Option<std::path::PathBuf> {
    let mut state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());

    // Don't save empty drafts
    let draft_text = state.voice_draft.trim();
    if draft_text.is_empty() {
        state.is_voice_active = false;
        return None;
    }

    // Save draft to file
    let path = codescribe_core::state::save_draft(draft_text);

    // Clear voice draft buffer
    state.voice_draft.clear();
    state.is_voice_active = false;

    // Update UI: clear voice draft view
    update_voice_draft_view_with_state(&mut state);

    // Refresh drafts list to show the new file
    populate_drafts_list(&mut state);

    info!("Voice draft finalized: {}", path.display());
    Some(path)
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
    // Clear both buffers
    state.manual_draft.clear();
    state.voice_draft.clear();
    state.attachments.clear();
    state.is_sending = false;
    state.is_voice_active = false;
    update_chat_view_with_state(&mut state, true);
    update_input_field_with_state(&mut state);
    update_voice_draft_view_with_state(&mut state);
    update_send_button_with_state(&mut state);
}

fn hide_voice_chat_overlay_impl() {
    unsafe {
        let mut state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(window_ptr) = state.window.take() {
            let window = window_ptr as Id;
            let _: () = msg_send![window, close];
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
    // This sends from manual_draft (left panel input field)
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

// ═══════════════════════════════════════════════════════════
// UI State Update Functions (used by handlers and mod.rs)
// ═══════════════════════════════════════════════════════════

pub fn update_chat_view_with_state(state: &mut VoiceChatOverlayState, force_rebuild: bool) {
    let Some(container_ptr) = state.bubble_container else {
        return;
    };
    let container = container_ptr as Id;

    // Check if we need to rebuild bubbles or just update the last one
    let bubble_count = state.bubble_views.len();
    let message_count = state.messages.len();

    // If streaming update to last message (same count, last is streaming)
    if !force_rebuild
        && bubble_count == message_count
        && message_count > 0
        && let Some(last_msg) = state.messages.last()
        && last_msg.is_streaming
        && let Some((_, text_label_ptr)) = state.bubble_views.last()
    {
        // Just update the last bubble's text
        let text_label = *text_label_ptr as Id;
        unsafe {
            update_bubble_text(text_label, &last_msg.text, true);
        }
        return;
    }

    // Full rebuild: clear existing bubbles
    unsafe {
        stack_view_clear(container);
    }
    state.bubble_views.clear();

    // Get max width for bubbles (left panel width - padding)
    let max_bubble_width = 420.0; // ~left_panel_width - 30

    // Get action handler for Copy buttons
    let action_handler = state.action_handler.map(|ptr| ptr as Id);

    // Add bubbles in reverse order (newest first at top)
    // Use enumerate to track original message indices for Copy buttons
    let messages_count = state.messages.len();
    for (rev_idx, message) in state.messages.iter().rev().enumerate() {
        // Convert reversed index back to original index
        let original_idx = messages_count - 1 - rev_idx;

        let role = match message.role {
            ChatRole::User => BubbleRole::User,
            ChatRole::Assistant => BubbleRole::Assistant,
            ChatRole::System => BubbleRole::System,
        };

        // Only show Copy button for completed messages (not streaming)
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
            .bubble_views
            .push((bubble_view as usize, text_label as usize));
    }

    // Scroll to top if forced (newest messages are at top)
    if force_rebuild && let Some(scroll_view_ptr) = state.scroll_view {
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
        if let Some(input_ptr) = state.input_field {
            let input_field = input_ptr as Id;
            let ns_string_class = Class::get("NSString").unwrap();
            // Manual input field shows manual_draft (left panel)
            let mut c_str = state.manual_draft.as_bytes().to_vec();
            c_str.push(0);
            let ns_str: Id = msg_send![ns_string_class, stringWithUTF8String: c_str.as_ptr()];
            let _: () = msg_send![input_field, setStringValue: ns_str];
        }
    }
}

pub fn update_send_button_with_state(state: &mut VoiceChatOverlayState) {
    unsafe {
        if let Some(send_ptr) = state.send_button {
            let send_button = send_ptr as Id;
            // Send button enabled when manual_draft has content
            let enabled = !state.is_sending && !state.manual_draft.trim().is_empty();
            let _: () = msg_send![send_button, setEnabled: enabled];
        }
    }
}

pub fn update_voice_draft_view_with_state(state: &mut VoiceChatOverlayState) {
    unsafe {
        if let Some(view_ptr) = state.voice_draft_view {
            let text_view = view_ptr as Id;
            let ns_string_class = Class::get("NSString").unwrap();
            let mut c_str = state.voice_draft.as_bytes().to_vec();
            c_str.push(0);
            let ns_str: Id = msg_send![ns_string_class, stringWithUTF8String: c_str.as_ptr()];
            let _: () = msg_send![text_view, setString: ns_str];
        }
    }
}

/// Populate the drafts list from ~/.codescribe/drafts/
pub fn populate_drafts_list(state: &mut VoiceChatOverlayState) {
    let Some(container_ptr) = state.drafts_container else {
        return;
    };
    let container = container_ptr as Id;

    // Clear existing items
    unsafe {
        stack_view_clear(container);
    }
    state.draft_files.clear();
    state.selected_draft_index = None;

    // Get drafts directory
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
    let drafts_dir = std::path::PathBuf::from(home).join(".codescribe/drafts");

    // List and cache draft files
    state.draft_files = list_draft_files(&drafts_dir);

    // Create UI row for each draft file
    for (index, path) in state.draft_files.iter().enumerate() {
        let filename = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("unknown");

        // Create a simple label for the draft
        let row = create_draft_row(filename, index);
        unsafe {
            stack_view_add(container, row);
        }
    }

    // Select first draft if available
    if !state.draft_files.is_empty() {
        state.selected_draft_index = Some(0);
    }

    info!("Populated {} drafts", state.draft_files.len());
}

/// Create a row for a draft file in the list
fn create_draft_row(filename: &str, _index: usize) -> Id {
    use core_graphics::geometry::{CGRect, CGSize};

    unsafe {
        let ns_text_field = Class::get("NSTextField").unwrap();
        let ns_color = Class::get("NSColor").unwrap();
        let ns_font = Class::get("NSFont").unwrap();

        // Simple text field showing filename with icon
        let display_text = format!("📄 {}", filename);

        let row_frame = CGRect {
            origin: CGPoint { x: 0.0, y: 0.0 },
            size: CGSize {
                width: 260.0,
                height: 24.0,
            },
        };

        let row: Id = msg_send![ns_text_field, alloc];
        let row: Id = msg_send![row, initWithFrame: row_frame];

        let _: () = msg_send![row, setBezeled: false];
        let _: () = msg_send![row, setEditable: false];
        let _: () = msg_send![row, setSelectable: true];
        let _: () = msg_send![row, setDrawsBackground: false];

        // White text
        let white: Id = msg_send![ns_color, whiteColor];
        let _: () = msg_send![row, setTextColor: white];

        // Small font
        let font: Id = msg_send![ns_font, systemFontOfSize: 11.0f64];
        let _: () = msg_send![row, setFont: font];

        // Set text
        let text = ns_string(&display_text);
        let _: () = msg_send![row, setStringValue: text];

        row
    }
}

/// Clear all overlay state (called when window closes)
pub fn clear_overlay_state(state: &mut VoiceChatOverlayState) {
    state.window = None;
    state.window_delegate = None;
    state.scroll_view = None;
    state.bubble_container = None;
    state.bubble_views.clear();
    state.status_field = None;
    state.input_field = None;
    state.send_button = None;
    state.attach_button = None;
    state.auto_send_checkbox = None;
    state.action_handler = None;
    state.voice_draft_view = None;
    state.voice_draft_header = None;
    state.voice_send_button = None;
    state.voice_use_button = None;
    state.collapse_button = None;
    state.tab_bar = None;
    state.drafts_scroll_view = None;
    state.drafts_container = None;
    state.draft_files.clear();
    state.selected_draft_index = None;
    state.settings_scroll_view = None;
    state.settings_container = None;
    state.ai_formatting_checkbox = None;
    state.edit_buttons_container = None;
    state.messages.clear();
    state.manual_draft.clear();
    state.voice_draft.clear();
    state.attachments.clear();
    state.is_sending = false;
    state.is_voice_active = false;
}
