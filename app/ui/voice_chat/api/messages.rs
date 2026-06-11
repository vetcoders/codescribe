//! Chat message lifecycle: streaming deltas, finalization, bubbles and chat view updates.

use super::*;

/// Append a delta to the user draft message (streaming transcription)
pub fn append_voice_chat_user_delta(delta: &str) {
    let delta_owned = delta.to_string();
    Queue::main().exec_async(move || {
        run_when_overlay_unlocked(move || append_voice_chat_user_delta_impl(&delta_owned));
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
        run_when_overlay_unlocked(move || append_voice_chat_assistant_delta_impl(&delta_owned));
    });
}

/// Append a delta to the live agent reasoning summary (streaming).
///
/// Reuses the exact same proven append path as the assistant/user lanes
/// (`TranscriptDelta::apply` + `get_or_create_streaming_message_index`), only on
/// the dedicated `Reasoning` lane — so the model's thinking is shown live instead
/// of a silent spinner. NOT mixed into the assistant text.
pub fn append_voice_chat_reasoning_delta(delta: &str) {
    let delta_owned = delta.to_string();
    Queue::main().exec_async(move || {
        run_when_overlay_unlocked(move || append_voice_chat_reasoning_delta_impl(&delta_owned));
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
        state.active_reasoning_stream_index = None;
        clear_agent_thinking_state(&mut state);
        let mode = message_mode_label(&state);
        state.messages.push(ChatMessage {
            role: ChatRole::System,
            text: text_owned.clone(),
            is_streaming: false,
            is_collapsed: false,
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
            is_collapsed: false,
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
            is_collapsed: false,
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

/// Mark that the agent is currently thinking/reasoning after a voice transcript was sent.
/// Used to drive "Thinking..." UI in the Agent tab of the voice chat overlay.
pub fn set_voice_chat_agent_thinking(thinking: bool) {
    Queue::main().exec_async(move || {
        let mut state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
        apply_agent_thinking(&mut state, thinking);
    });
}

/// Apply agent-thinking state on an already-held `OVERLAY_STATE` guard, then
/// refresh status + send button in place. Must NOT lock `OVERLAY_STATE`: the
/// caller already holds it.
///
/// DEADLOCK PREVENTION: the previous inline version called
/// `update_voice_chat_status_impl` here, which re-locked the same non-reentrant
/// `std::sync::Mutex` on the main thread and froze it in `__psynch_mutexwait`
/// (overlay hang the moment Emil entered "Thinking…" after a voice handoff).
/// Status is now applied via `apply_voice_chat_status`, which operates on the
/// held guard without re-locking.
pub fn apply_agent_thinking(state: &mut VoiceChatOverlayState, thinking: bool) {
    if thinking {
        state.is_agent_thinking = true;
        state.status_base_text = "Thinking…".to_string();
    } else {
        clear_agent_thinking_state(state);
    }
    let base = state.status_base_text.clone();
    apply_voice_chat_status(state, &base);
    update_send_button_with_state(state);
}

pub fn handle_message_bubble_click_from_recognizer(sender: Id) {
    if sender.is_null() {
        return;
    }
    let recognizer_ptr = sender as usize;
    let mut state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
    let Some(index) = state
        .agent_bubble_click_recognizers
        .iter()
        .find(|(ptr, _)| *ptr == recognizer_ptr)
        .map(|(_, index)| *index)
    else {
        debug!("Bubble click did not resolve to a message recognizer");
        return;
    };

    let Some(message) = state.messages.get_mut(index) else {
        debug!("Bubble click pointed outside message list");
        return;
    };

    match message.role {
        ChatRole::Reasoning => {
            message.is_collapsed = !message.is_collapsed;
            update_chat_view_with_state(&mut state, false);
        }
        ChatRole::Assistant if !message.text.is_empty() => {
            let text = message.text.clone();
            drop(state);
            copy_to_clipboard(&text);
            info!("Copied assistant bubble to clipboard");
        }
        _ => {
            debug!("Bubble click had no action for role {:?}", message.role);
        }
    }
}

/// Minimum interval between layout passes during streaming (prevents main-thread saturation).
pub const DELTA_LAYOUT_THROTTLE: Duration = Duration::from_millis(50);
pub const SCROLL_BOTTOM_THRESHOLD: f64 = 24.0;

pub fn should_autoscroll(scroll_pinned: bool) -> bool {
    scroll_pinned
}

pub fn scrolled_to_bottom_math(
    visible_y: f64,
    visible_height: f64,
    document_height: f64,
    threshold: f64,
) -> bool {
    let visible_max_y = visible_y.max(0.0) + visible_height.max(0.0);
    let threshold = threshold.max(0.0);
    let bottom_y = (document_height.max(0.0) - threshold).max(0.0);
    visible_max_y + f64::EPSILON >= bottom_y
}

pub unsafe fn is_scrolled_to_bottom(agent_scroll: Id) -> bool {
    unsafe {
        if agent_scroll.is_null() {
            return true;
        }
        let document_view: Id = msg_send![agent_scroll, documentView];
        if document_view.is_null() {
            return true;
        }
        let visible: CGRect = msg_send![agent_scroll, documentVisibleRect];
        let document_frame: CGRect = msg_send![document_view, frame];
        scrolled_to_bottom_math(
            visible.origin.y,
            visible.size.height,
            document_frame.size.height,
            SCROLL_BOTTOM_THRESHOLD,
        )
    }
}

pub fn latest_message_is_streaming(state: &VoiceChatOverlayState) -> bool {
    state
        .messages
        .last()
        .map(|message| message.is_streaming)
        .unwrap_or(false)
}

pub fn update_latest_pill_visibility(state: &VoiceChatOverlayState) {
    let Some(button_ptr) = state.agent_latest_button else {
        return;
    };
    let should_show = state.active_tab == Tab::Agent
        && !state.scroll_pinned
        && latest_message_is_streaming(state);
    unsafe {
        crate::ui_helpers::set_hidden(button_ptr as Id, !should_show);
    }
}

pub unsafe fn scroll_agent_to_bottom(
    last_bubble_ptr: Option<usize>,
    scroll_view_ptr: Option<usize>,
) {
    unsafe {
        if let Some(bubble_ptr) = last_bubble_ptr {
            let bubble = bubble_ptr as Id;
            let bounds: CGRect = msg_send![bubble, bounds];
            let y = (bounds.size.height - 2.0).max(0.0);
            let rect = CGRect::new(&CGPoint::new(0.0, y), &CGSize::new(bounds.size.width, 2.0));
            let _: () = msg_send![bubble, scrollRectToVisible: rect];
            return;
        }

        if let Some(scroll_view_ptr) = scroll_view_ptr {
            let scroll_view = scroll_view_ptr as Id;
            let content_view: Id = msg_send![scroll_view, contentView];
            if !content_view.is_null() {
                let _: () = msg_send![content_view, scrollToPoint: CGPoint::new(0.0, 0.0)];
                let _: () = msg_send![scroll_view, reflectScrolledClipView: content_view];
            }
        }
    }
}

pub fn handle_agent_scroll_live() {
    let scroll_view_ptr = {
        let state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
        state.agent_scroll_view
    };
    let Some(scroll_view_ptr) = scroll_view_ptr else {
        return;
    };

    let pinned = unsafe { is_scrolled_to_bottom(scroll_view_ptr as Id) };
    let mut state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
    if state.agent_scroll_view != Some(scroll_view_ptr) {
        return;
    }
    state.scroll_pinned = pinned;
    update_latest_pill_visibility(&state);
}

pub fn pin_agent_scroll_to_latest_impl() {
    let (last_bubble, scroll_view) = {
        let mut state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
        state.scroll_pinned = true;
        update_latest_pill_visibility(&state);
        (
            state
                .agent_bubble_views
                .last()
                .map(|(bubble_ptr, _)| *bubble_ptr),
            state.agent_scroll_view,
        )
    };

    unsafe {
        scroll_agent_to_bottom(last_bubble, scroll_view);
    }
}

pub fn resolve_delta_index(
    state: &VoiceChatOverlayState,
    requested: Option<usize>,
) -> Option<usize> {
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

pub fn apply_delta_and_layout(state: &mut VoiceChatOverlayState, updated_index: Option<usize>) {
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
                    run_when_overlay_unlocked(|| {
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
                    })
                });
            });
        }
    }
}

pub fn append_voice_chat_user_delta_impl(delta: &str) {
    let mut state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
    ensure_agent_tab_visible(&mut state);
    let idx = get_or_create_streaming_message_index(&mut state, ChatRole::User);
    if let Some(msg) = state.messages.get_mut(idx) {
        codescribe_core::pipeline::contracts::TranscriptDelta::from_raw(delta).apply(&mut msg.text);
        msg.is_streaming = true;
        msg.is_collapsed = false;
    }
    apply_delta_and_layout(&mut state, Some(idx));
}

pub fn append_voice_chat_assistant_delta_impl(delta: &str) {
    let mut state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
    ensure_agent_tab_visible(&mut state);

    // First assistant token → stop the "Thinking" indicator and finalize any live
    // reasoning lane (collapse it to its title — the answer is starting).
    clear_agent_thinking_state(&mut state);
    finalize_streaming_reasoning(&mut state);

    let idx = get_or_create_streaming_message_index(&mut state, ChatRole::Assistant);
    if let Some(msg) = state.messages.get_mut(idx) {
        codescribe_core::pipeline::contracts::TranscriptDelta::from_raw(delta).apply(&mut msg.text);
        msg.is_streaming = true;
    }
    apply_delta_and_layout(&mut state, Some(idx));
}

pub fn append_voice_chat_reasoning_delta_impl(delta: &str) {
    let mut state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
    ensure_agent_tab_visible(&mut state);

    // Reasoning IS the live "thinking" — surface it instead of a silent spinner.
    // Do NOT clear the thinking indicator here; the first ASSISTANT token does that.
    let idx = get_or_create_streaming_message_index(&mut state, ChatRole::Reasoning);
    if let Some(msg) = state.messages.get_mut(idx) {
        codescribe_core::pipeline::contracts::TranscriptDelta::from_raw(delta).apply(&mut msg.text);
        msg.is_streaming = true;
    }
    apply_delta_and_layout(&mut state, Some(idx));
}

/// Stop streaming on any live reasoning message (called when the assistant answer
/// begins). The reasoning entry stays in the log as a finished, collapsible item.
pub fn finalize_streaming_reasoning(state: &mut VoiceChatOverlayState) {
    for msg in state.messages.iter_mut() {
        if msg.role == ChatRole::Reasoning && msg.is_streaming {
            msg.is_streaming = false;
            msg.is_collapsed = true;
        }
    }
    state.active_reasoning_stream_index = None;
}

pub fn display_text_for_message(message: &ChatMessage) -> String {
    if message.role == ChatRole::Reasoning && message.is_collapsed {
        reasoning_summary_header(message)
    } else if message.is_streaming && message.text.is_empty() {
        "• • •".to_string()
    } else if message.is_streaming {
        format!("{} …", message.text)
    } else {
        message.text.clone()
    }
}

pub fn bubble_text_for_message(message: &ChatMessage) -> String {
    if message.role == ChatRole::Reasoning && message.is_collapsed {
        reasoning_summary_header(message)
    } else {
        message.text.clone()
    }
}

pub fn bubble_streaming_for_message(message: &ChatMessage) -> bool {
    !(message.role == ChatRole::Reasoning && message.is_collapsed) && message.is_streaming
}

pub fn message_render_role(message: &ChatMessage) -> BubbleRole {
    match message.role {
        ChatRole::User => BubbleRole::User,
        ChatRole::Assistant => BubbleRole::Assistant,
        ChatRole::System | ChatRole::Reasoning => BubbleRole::System,
    }
}

pub fn message_render_mode_for(
    overrides: &std::collections::HashMap<usize, RenderMode>,
    index: usize,
    message: &ChatMessage,
) -> RenderMode {
    overrides.get(&index).copied().unwrap_or_else(|| {
        streaming_render_mode(
            bubble_streaming_for_message(message),
            message_render_role(message),
        )
    })
}

pub fn reasoning_summary_header(message: &ChatMessage) -> String {
    let elapsed = SystemTime::now()
        .duration_since(message.timestamp)
        .unwrap_or_default()
        .as_secs()
        .max(1);
    let chars = message.text.chars().count();
    format!("Reasoning · {elapsed}s / {chars} chars")
}

pub fn message_mode_label(state: &VoiceChatOverlayState) -> String {
    if !matches!(state.conversation_state, ConversationModeState::Inactive) {
        "Moshi".to_string()
    } else if state.auto_send_enabled {
        "AI".to_string()
    } else {
        "Manual".to_string()
    }
}

pub fn message_role_label(role: ChatRole) -> &'static str {
    match role {
        ChatRole::User => "You",
        ChatRole::Assistant => "Assistant",
        ChatRole::System => "System",
        ChatRole::Reasoning => "Reasoning",
    }
}

pub fn message_metadata(message: &ChatMessage) -> String {
    let when: DateTime<Local> = message.timestamp.into();
    let time = when.format("%H:%M").to_string();
    let role = message_role_label(message.role);
    if let Some(mode) = message.mode.as_ref() {
        format!("{role} · {time} · {mode}")
    } else {
        format!("{role} · {time}")
    }
}

pub unsafe fn agent_max_width(state: &VoiceChatOverlayState) -> f64 {
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

pub fn update_cached_stack_height(
    old_height: f64,
    new_height: f64,
    cached_height: Option<f64>,
) -> Option<f64> {
    cached_height.map(|height| (height - old_height.max(0.0) + new_height.max(0.0)).max(1.0))
}

unsafe fn view_height(view: Id) -> f64 {
    unsafe {
        let frame: CGRect = msg_send![view, frame];
        frame.size.height.max(0.0)
    }
}

unsafe fn measure_agent_stack_height(container: Id) -> f64 {
    unsafe {
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

        total_h
    }
}

pub unsafe fn sync_agent_document_view_size(
    state: &mut VoiceChatOverlayState,
    max_width: f64,
    streaming_height_delta: Option<(f64, f64)>,
) {
    let Some(container_ptr) = state.agent_container else {
        return;
    };
    let container = container_ptr as Id;

    let total_h = if let Some((old_height, new_height)) = streaming_height_delta
        && let Some(cached) =
            update_cached_stack_height(old_height, new_height, state.cached_agent_stack_height)
    {
        state.cached_agent_stack_height = Some(cached);
        cached
    } else {
        let measured = unsafe { measure_agent_stack_height(container) };
        state.cached_agent_stack_height = Some(measured);
        measured
    };

    unsafe {
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
}

pub fn try_update_message_view_in_place(state: &mut VoiceChatOverlayState, index: usize) -> bool {
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
        let render_mode = message_render_mode_for(&state.message_render_modes, index, message);

        let container = bubble_ptr as Id;
        let label = label_ptr as Id;
        let old_container_height = view_height(container);
        let bubble_text = bubble_text_for_message(message);
        let bubble_is_streaming = bubble_streaming_for_message(message);
        let bubble_role = message_render_role(message);
        update_bubble_text_with_render_mode(
            label,
            &bubble_text,
            bubble_role,
            bubble_is_streaming,
            message.is_error,
            render_mode,
        );
        let display_text = display_text_for_message(message);
        resize_bubble_container_for_text(container, label, &display_text);
        let new_container_height = view_height(container);
        let last_streaming_bubble_delta =
            if bubble_is_streaming && index + 1 == state.agent_bubble_views.len() {
                Some((old_container_height, new_container_height))
            } else {
                None
            };
        let max_width = agent_max_width(state);
        sync_agent_document_view_size(state, max_width, last_streaming_bubble_delta);

        update_latest_pill_visibility(state);

        if should_autoscroll(state.scroll_pinned) && index + 1 == state.agent_bubble_views.len() {
            scroll_agent_to_bottom(Some(container as usize), state.agent_scroll_view);
        }
        true
    }
}

pub fn toggle_message_render_mode_impl(index: usize) {
    let mut state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
    let Some(message) = state.messages.get(index) else {
        debug!("Render toggle pointed outside message list");
        return;
    };
    if !matches!(
        message.role,
        ChatRole::Assistant | ChatRole::System | ChatRole::Reasoning
    ) {
        debug!("Render toggle ignored for role {:?}", message.role);
        return;
    }

    let current = message_render_mode_for(&state.message_render_modes, index, message);
    let next = next_render_mode(current);
    state.message_render_modes.insert(index, next);

    if !try_update_message_view_in_place(&mut state, index) {
        update_chat_view_with_state(&mut state, false);
    }
}

pub fn release_agent_bubble_click_recognizers(state: &mut VoiceChatOverlayState) {
    let recognizers = std::mem::take(&mut state.agent_bubble_click_recognizers);
    for (recognizer_ptr, _) in recognizers {
        unsafe {
            crate::ui_helpers::release_object(recognizer_ptr as Id);
        }
    }
}

pub unsafe fn attach_message_bubble_click_recognizer(
    state: &mut VoiceChatOverlayState,
    bubble: Id,
    message_index: usize,
) {
    unsafe {
        let Some(target_ptr) = state.action_handler else {
            return;
        };
        let Some(ns_click_gesture) = Class::get("NSClickGestureRecognizer") else {
            return;
        };
        let recognizer: Id = msg_send![ns_click_gesture, alloc];
        if recognizer.is_null() {
            return;
        }
        let recognizer: Id = msg_send![
                recognizer,
                initWithTarget: target_ptr as Id
                action: sel!(onAssistantBubbleClick:)
        ];
        if recognizer.is_null() {
            return;
        }
        let _: () = msg_send![recognizer, setNumberOfClicksRequired: 1_isize];
        let _: () = msg_send![bubble, addGestureRecognizer: recognizer];
        state
            .agent_bubble_click_recognizers
            .push((recognizer as usize, message_index));
    }
}

pub fn finalize_user_message_impl(text: &str) {
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
                is_collapsed: false,
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
            is_collapsed: false,
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

pub fn finalize_user_message_state_only_impl() {
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

pub fn finalize_assistant_message_impl(text: &str, is_error: bool) {
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
                is_collapsed: false,
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
            is_collapsed: false,
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
    clear_agent_thinking_state(&mut state);
    state.is_sending = false;
    update_chat_view_with_state(&mut state, true);
    update_send_button_with_state(&mut state);
}

pub fn finalize_assistant_message_state_only_impl(is_error: bool) {
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
    clear_agent_thinking_state(&mut state);
    state.is_sending = false;
    update_chat_view_with_state(&mut state, true);
    update_send_button_with_state(&mut state);
}

pub fn clear_agent_thinking_state(state: &mut VoiceChatOverlayState) {
    if state.is_agent_thinking {
        state.is_agent_thinking = false;
        if state.status_base_text == "Thinking…" {
            state.status_base_text = "Ready".to_string();
        }
        state.status_text = compose_runtime_status_text(
            &state.status_base_text,
            state.is_agent_degraded,
            state.runtime_degraded_reason.as_deref(),
        );
        state.status_kind =
            status_kind_for_runtime(&state.status_base_text, state.is_agent_degraded);
        apply_status_pill(state);
        let _ = crate::tray::update_tray_status(state.status_kind.to_tray());
    }
}

pub fn ensure_agent_tab_visible(state: &mut VoiceChatOverlayState) {
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

pub fn handoff_transcript_to_chat_impl(transcript: &str) {
    let callback = {
        let mut state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
        ensure_agent_tab_visible(&mut state);
        state.active_user_stream_index = None;
        let mode = message_mode_label(&state);
        state.messages.push(ChatMessage {
            role: ChatRole::User,
            text: transcript.to_string(),
            is_streaming: false,
            is_collapsed: false,
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

pub fn active_stream_index_mut(
    state: &mut VoiceChatOverlayState,
    role: ChatRole,
) -> Option<&mut Option<usize>> {
    match role {
        ChatRole::User => Some(&mut state.active_user_stream_index),
        ChatRole::Assistant => Some(&mut state.active_assistant_stream_index),
        ChatRole::Reasoning => Some(&mut state.active_reasoning_stream_index),
        ChatRole::System => None,
    }
}

pub fn active_stream_index(state: &VoiceChatOverlayState, role: ChatRole) -> Option<usize> {
    match role {
        ChatRole::User => state.active_user_stream_index,
        ChatRole::Assistant => state.active_assistant_stream_index,
        ChatRole::Reasoning => state.active_reasoning_stream_index,
        ChatRole::System => None,
    }
}

pub fn is_valid_stream_message(state: &VoiceChatOverlayState, idx: usize, role: ChatRole) -> bool {
    state
        .messages
        .get(idx)
        .map(|msg| msg.role == role && msg.is_streaming)
        .unwrap_or(false)
}

pub fn get_or_create_streaming_message_index(
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
        is_collapsed: false,
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

pub fn update_chat_view_with_state(state: &mut VoiceChatOverlayState, scroll_to_bottom: bool) {
    unsafe {
        let Some(container_ptr) = state.agent_container else {
            return;
        };
        let container = container_ptr as Id;
        release_agent_bubble_click_recognizers(state);
        stack_view_clear(container);
        state.agent_bubble_views.clear();
        let message_len = state.messages.len();
        state
            .message_render_modes
            .retain(|index, _| *index < message_len);
        state.cached_agent_stack_height = None;

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
        let message_count = state.messages.len();
        for index in 0..message_count {
            let message = &state.messages[index];
            let message_role = message.role;
            let message_text = bubble_text_for_message(message);
            let message_is_streaming = bubble_streaming_for_message(message);
            let message_is_error = message.is_error;
            let message_render_mode =
                message_render_mode_for(&state.message_render_modes, index, message);
            let message_metadata = if message_role == ChatRole::Reasoning && message.is_collapsed {
                None
            } else {
                Some(message_metadata(message))
            };
            let role = message_render_role(message);
            let (bubble, text_label) = create_bubble_view(BubbleConfig {
                text: message_text,
                role,
                max_width,
                font_size: base_font * zoom,
                is_streaming: message_is_streaming,
                is_error: message_is_error,
                render_mode: Some(message_render_mode),
                metadata: message_metadata,
                message_index: Some(index),
                copy_action_target: state.action_handler.map(|p| p as Id),
            });
            if matches!(message_role, ChatRole::Assistant | ChatRole::Reasoning) {
                attach_message_bubble_click_recognizer(state, bubble, index);
            }
            stack_view_add(container, bubble);
            last_bubble = Some(bubble);
            state
                .agent_bubble_views
                .push((bubble as usize, text_label as usize));

            // Add commit/discard action bar for draft user messages
            if message_role == ChatRole::User
                && index == message_count - 1
                && !state.auto_send_enabled
                && !state.is_sending
            {
                let action_bar = create_commit_action_bar(state.action_handler);
                stack_view_add(container, action_bar);
            }
        }

        // Ensure the document view size matches its arranged subviews; otherwise scrolling can
        // be disabled and long messages will just "grow" out of view.
        sync_agent_document_view_size(state, max_width, None);

        update_latest_pill_visibility(state);

        if scroll_to_bottom && should_autoscroll(state.scroll_pinned) {
            if let Some(bubble) = last_bubble {
                scroll_agent_to_bottom(Some(bubble as usize), state.agent_scroll_view);
            } else if let Some(scroll_view_ptr) = state.agent_scroll_view {
                scroll_agent_to_bottom(None, Some(scroll_view_ptr));
            }
        }
    }
}
