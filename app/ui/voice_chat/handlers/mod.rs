//! Action handlers for voice chat overlay
//!
//! Contains Objective-C class registration and action handler functions,
//! decomposed by responsibility: class registration, window geometry and
//! zoom, attachment intake, connector fetches, modal dialogs, popup menus
//! and plain action trampolines.

mod actions;
mod attachments;
mod classes;
mod connectors;
mod dialogs;
mod menus;
mod window;

#[cfg(test)]
mod tests;

pub use actions::*;
pub use attachments::*;
pub use classes::*;
pub use connectors::*;
pub use dialogs::*;
pub use menus::*;
pub use window::*;

use core_graphics::base::CGFloat;
use core_graphics::geometry::{CGPoint, CGRect, CGSize};
use dispatch::Queue;
use objc::declare::ClassDecl;
use objc::runtime::{Class, Object, Sel};
use objc::{msg_send, sel, sel_impl};
use std::path::PathBuf;
use std::sync::Once;
use tracing::{debug, info};

use codescribe_core::attachment::{Attachment, AttachmentSource, AttachmentStore};
use codescribe_core::config::UserSettings;

use crate::config::Config;
use crate::ui_helpers::{
    clamp_overlay_position, copy_to_clipboard, get_text_field_string, ns_string, set_hidden,
    set_text_field_string,
};

use super::api::{
    clear_overlay_state, commit_last_user_message_impl, discard_last_message_impl, filter_drawer,
    handle_agent_scroll_live, handle_card_copy, handle_card_delete, handle_card_edit,
    handle_card_favorite, handle_card_restore, handle_message_bubble_click_from_recognizer,
    pin_agent_scroll_to_latest_impl, reflow_agent_after_resize_impl,
    reflow_overlay_after_resize_impl, render_attachment_chips, send_draft_message_impl,
    start_new_thread_impl, toggle_drawer_favorites_only_impl, update_active_tab_impl,
    update_attach_button_ui,
};
use super::state::{ChatRole, OVERLAY_STATE, Tab, VoiceChatOverlayState};

// Type alias for Objective-C object pointers
pub use crate::ui_helpers::Id;
