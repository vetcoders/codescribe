//! Public API and internal helpers for voice chat overlay
//!
//! Contains all the public functions for controlling the overlay and
//! internal helper functions for state updates.

mod drawer;
mod export;
mod lifecycle;
mod messages;
mod send;
mod status;
mod tabs;
#[cfg(test)]
mod tests;

pub use drawer::*;
pub use export::*;
pub use lifecycle::*;
pub use messages::*;
pub use send::*;
pub use status::*;
pub use tabs::*;

use super::handlers::clear_search_field;
use super::state::{
    ChatMessage, ChatRole, ConversationModeState, DrawerEntry, DrawerEntrySource, OVERLAY_STATE,
    SEND_CALLBACK, Tab, TranscriptionMode, VoiceChatOverlayState,
};
use crate::ui::shared::status::{UiStatus, status_from_detail};
use crate::ui_helpers::{
    BubbleConfig, BubbleRole, LabelConfig, NSEdgeInsets, RenderMode, add_subview,
    apply_tafla_surface, button_set_action, button_style, chat_header_layout, color_label,
    color_rgba, color_secondary_label, copy_to_clipboard, create_bubble_view, create_button,
    create_label, get_text_field_string, get_text_view_string, layout_region_frame_for_view,
    next_render_mode, ns_string, open_file_in_editor, resize_bubble_container_for_text,
    set_button_symbol, set_text_field_string, set_text_view_string, set_tooltip, stack_view_add,
    stack_view_clear, streaming_render_mode, ui_colors, ui_tokens,
    update_bubble_text_with_render_mode, window_set_alpha, window_show,
};
use chrono::{DateTime, Local};
use codescribe_core::agent::{Thread, ThreadIndex, ThreadStore};
use codescribe_core::attachment::Attachment;
use core_graphics::geometry::{CGPoint, CGRect, CGSize};
use dispatch::Queue;
use objc::runtime::{Class, Object, Sel};
use objc::{msg_send, sel, sel_impl};
use std::collections::HashSet;
use std::path::PathBuf;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tracing::{debug, info, warn};
// Type alias for Objective-C object pointers
pub use crate::ui_helpers::Id;

// ═══════════════════════════════════════════════════════════
// Public API
// ═══════════════════════════════════════════════════════════

/// Run `f` on the main queue only when `OVERLAY_STATE` is not already held.
///
/// DEADLOCK PREVENTION: AppKit can spin a nested run-loop while an OUTER frame
/// on this same main thread still holds `OVERLAY_STATE` (see module docs). A
/// queued block that then calls `.lock()` self-deadlocks the non-reentrant
/// Mutex — this froze the whole app (sample: main thread 100% in
/// __psynch_mutexwait inside _dispatch_main_queue_drain). Instead of blocking,
/// probe with `try_lock`; if busy, requeue after a short delay and let the
/// holder finish its AppKit call.
pub fn run_when_overlay_unlocked<F>(f: F)
where
    F: FnOnce() + Send + 'static,
{
    match OVERLAY_STATE.try_lock() {
        Ok(guard) => {
            drop(guard);
            f();
        }
        Err(std::sync::TryLockError::Poisoned(err)) => {
            drop(err);
            f();
        }
        Err(std::sync::TryLockError::WouldBlock) => {
            Queue::main().exec_after(Duration::from_millis(5), move || {
                run_when_overlay_unlocked(f);
            });
        }
    }
}
