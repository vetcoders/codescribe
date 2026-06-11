//! Overlay visibility, hide/teardown and state clearing.

use super::*;

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
pub struct ReleasedOverlayHandles {
    window_delegate: Option<usize>,
    action_handler: Option<usize>,
    window: Option<usize>,
}

/// Drain the three owned ObjC handles out of the overlay state and clear all
/// other fields. Returns the handles for the caller to release after dropping
/// the state lock. Calling code MUST eventually `release` each `Some(ptr)`
/// exactly once or leak the underlying object.
pub fn take_handles_and_clear_overlay_state(
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

pub fn hide_voice_chat_overlay_impl() {
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
        }

        std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(200));
            dispatch::Queue::main().exec_async(move || {
                debug!(
                    "voice_chat teardown: closing NSWindow (ptr={:#x}) + delegates",
                    window_ptr
                );
                // SAFETY: each pointer was obtained from `[cls new]` (or
                // equivalent alloc/init pair) on the main thread, retained at
                // +1, and is still alive because `take_handles_and_clear_overlay_state`
                // is the unique teardown site. We are on the main thread (dispatch
                // queue is main). The explicit autoreleasepool scope ensures
                // autoreleased temporaries from AppKit's `windowWillClose` /
                // CoreAnimation fade cleanup drain in-scope, before this closure
                // exits — without it, pendingowe autoreleases survive into the
                // next runloop tick's pool pop and can hit pointers freed by
                // these same release calls, producing EXC_BAD_ACCESS in
                // `objc_release` during `_CFAutoreleasePoolPop` (observed as
                // SIGSEGV on macOS Tahoe beta, 2026-05-10 and 2026-05-13).
                objc2::rc::autoreleasepool(|_pool| unsafe {
                    let window = window_ptr as Id;
                    // Close window FIRST so AppKit dispatches `windowWillClose`
                    // delegate callbacks while delegate + action handler are
                    // still alive. Then release deps. The shared shell policy
                    // sets `released_when_closed = false`, so `window_close`
                    // does NOT balance the +1 retain — caller releases below.
                    crate::ui_helpers::window_close(window);
                    if let Some(ptr) = handles.window_delegate {
                        crate::ui_helpers::release_object(ptr as Id);
                    }
                    if let Some(ptr) = handles.action_handler {
                        crate::ui_helpers::release_object(ptr as Id);
                    }
                    crate::ui_helpers::release_object(window);
                });
            });
        });
    } else {
        // If there was no window, we still need to release the delegate and action handler
        unsafe {
            if let Some(ptr) = handles.window_delegate {
                let _: () = msg_send![ptr as Id, release];
            }
            if let Some(ptr) = handles.action_handler {
                let _: () = msg_send![ptr as Id, release];
            }
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
    release_agent_bubble_click_recognizers(state);
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
    state.active_reasoning_stream_index = None;
    state.is_sending = false;
    state.manual_draft.clear();
    state.conversation_state = ConversationModeState::Inactive;
}
