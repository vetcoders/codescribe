//! Voice Chat UI overlay for displaying streaming responses.
//!
//! This module provides a floating overlay window that:
//! - Shows drawer entries for transcriptions
//! - Displays an agent tab with chat bubbles
//! - Keeps existing streaming response pipeline

mod api;
mod handlers;
mod state;

// Re-export public API
pub use api::{
    add_voice_chat_error_message, add_voice_chat_user_message, append_voice_chat_assistant_delta,
    clear_voice_chat_text, filter_drawer, hide_voice_chat_overlay, is_auto_send_enabled,
    is_voice_chat_overlay_visible, refresh_drawer, reset_voice_chat_activity,
    send_voice_chat_draft, set_active_tab, set_voice_chat_send_callback, set_voice_chat_sending,
    set_voice_chat_text, update_voice_chat_status,
};
pub use state::{ChatMessage, ChatRole, DrawerEntry, Tab, TranscriptionMode};

use codescribe_core::config::{Config, OverlayPositionMode};
use core_graphics::geometry::{CGPoint, CGRect, CGSize};
use dispatch::Queue;
use objc::runtime::{Class, Object};
use objc::{msg_send, sel, sel_impl};
use objc2_app_kit::{
    NSBackingStoreType, NSVisualEffectBlendingMode, NSVisualEffectMaterial, NSVisualEffectState,
    NSWindowCollectionBehavior, NSWindowStyleMask,
};
use tracing::info;

use crate::ui_helpers::{
    add_subview, button_set_action, button_style, color_clear, color_white, create_button,
    create_label, create_segmented_control, create_vertical_stack_view, ns_string, set_hidden,
    window_set_alpha, window_show,
};

use api::{
    refresh_drawer_impl, set_active_tab_impl, update_chat_view_with_state,
    update_input_field_with_state, update_send_button_with_state,
};
use handlers::{action_handler_class, window_delegate_class};
use state::{OVERLAY_STATE, Tab as OverlayTab};

// Type alias for Objective-C object pointers
// SAFETY: raw Objective-C pointers used in AppKit FFI.
type Id = *mut Object;

// Window level constants
const NS_FLOATING_WINDOW_LEVEL: i64 = 3;

/// Show the voice chat overlay window
pub fn show_voice_chat_overlay() {
    Queue::main().exec_async(|| {
        show_voice_chat_overlay_impl();
    });
}

fn show_voice_chat_overlay_impl() {
    unsafe {
        let mut state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());

        // Reuse existing window if any
        if let Some(window_ptr) = state.window {
            let window = window_ptr as Id;
            window_show(window);
            info!("Voice chat overlay reused");
            return;
        }

        state.is_sending = false;

        let ns_window = Class::get("NSWindow").unwrap();
        let ns_text_field = Class::get("NSTextField").unwrap();
        let ns_visual_effect_view = Class::get("NSVisualEffectView").unwrap();
        let ns_scroll_view = Class::get("NSScrollView").unwrap();
        let ns_search_field = Class::get("NSSearchField").unwrap();

        // Get screen size to position the overlay
        let ns_screen = Class::get("NSScreen").unwrap();
        let main_screen: Id = msg_send![ns_screen, mainScreen];
        let visible_frame: CGRect = msg_send![main_screen, visibleFrame];

        // Load config for position logic
        let config = Config::load();

        let window_width = 450.0;
        let window_height = 520.0;
        let margin = 20.0;

        let (x, y) = match config.overlay_position_mode {
            OverlayPositionMode::SnappedTopRight => {
                let right_x = visible_frame.origin.x + visible_frame.size.width;
                let top_y = visible_frame.origin.y + visible_frame.size.height;
                (
                    right_x - window_width - margin,
                    top_y - window_height - margin,
                )
            }
            OverlayPositionMode::Custom => {
                let right_x = visible_frame.origin.x + visible_frame.size.width;
                let top_y = visible_frame.origin.y + visible_frame.size.height;
                let def_x = right_x - window_width - margin;
                let def_y = top_y - window_height - margin;
                (
                    config.overlay_custom_x.unwrap_or(def_x),
                    config.overlay_custom_y.unwrap_or(def_y),
                )
            }
        };

        let frame = CGRect::new(
            &CGPoint::new(x, y),
            &CGSize::new(window_width, window_height),
        );

        let window: Id = msg_send![ns_window, alloc];
        let style_mask = NSWindowStyleMask::Borderless | NSWindowStyleMask::FullSizeContentView;
        let backing = NSBackingStoreType::Buffered;
        let window: Id = msg_send![
            window,
            initWithContentRect: frame
            styleMask: style_mask
            backing: backing
            defer: false
        ];

        let _: () = msg_send![window, setTitleVisibility: 1];
        let _: () = msg_send![window, setTitlebarAppearsTransparent: true];
        let _: () = msg_send![window, setMovableByWindowBackground: true];
        let _: () = msg_send![window, setOpaque: false];
        let _: () = msg_send![window, setBackgroundColor: color_clear()];
        let _: () = msg_send![window, setHasShadow: true];
        let _: () = msg_send![window, setLevel: NS_FLOATING_WINDOW_LEVEL];
        let collection_behavior = NSWindowCollectionBehavior::CanJoinAllSpaces;
        let _: () = msg_send![window, setCollectionBehavior: collection_behavior];

        let delegate_class = window_delegate_class();
        let window_delegate: Id = msg_send![delegate_class, new];
        let _: () = msg_send![window, setDelegate: window_delegate];

        let content_view: Id = msg_send![window, contentView];

        let blur_view: Id = msg_send![ns_visual_effect_view, alloc];
        let blur_view: Id = msg_send![blur_view, initWithFrame: frame];
        let _: () = msg_send![blur_view, setMaterial: NSVisualEffectMaterial::HUDWindow];
        let _: () = msg_send![blur_view, setBlendingMode: NSVisualEffectBlendingMode::BehindWindow];
        let _: () = msg_send![blur_view, setState: NSVisualEffectState::Active];
        let _: () = msg_send![blur_view, setWantsLayer: true];
        let blur_layer: Id = msg_send![blur_view, layer];
        if !blur_layer.is_null() {
            let _: () = msg_send![blur_layer, setCornerRadius: 16.0f64];
        }
        add_subview(content_view, blur_view);

        // Header
        let header_height = 44.0;
        let header_frame = CGRect::new(
            &CGPoint::new(0.0, window_height - header_height),
            &CGSize::new(window_width, header_height),
        );
        let header_view: Id = msg_send![Class::get("NSView").unwrap(), alloc];
        let header_view: Id = msg_send![header_view, initWithFrame: header_frame];
        add_subview(blur_view, header_view);

        let title_frame = CGRect::new(&CGPoint::new(16.0, 12.0), &CGSize::new(140.0, 20.0));
        let title_label = create_label(crate::ui_helpers::LabelConfig {
            frame: title_frame,
            text: "CodeScribe".to_string(),
            font_size: 14.0,
            bold: true,
            text_color: color_white(0.9),
            ..Default::default()
        });
        add_subview(header_view, title_label);

        let tab_frame = CGRect::new(&CGPoint::new(170.0, 10.0), &CGSize::new(160.0, 24.0));
        let tab_control = create_segmented_control(tab_frame, &["Drawer", "Agent"]);
        add_subview(header_view, tab_control);

        let settings_frame = CGRect::new(
            &CGPoint::new(window_width - 70.0, 10.0),
            &CGSize::new(24.0, 24.0),
        );
        let settings_button = create_button(settings_frame, "⚙", button_style::INLINE);
        add_subview(header_view, settings_button);

        let close_frame = CGRect::new(
            &CGPoint::new(window_width - 36.0, 10.0),
            &CGSize::new(24.0, 24.0),
        );
        let close_button = create_button(close_frame, "×", button_style::INLINE);
        add_subview(header_view, close_button);

        // Drawer scroll view
        let drawer_frame = CGRect::new(
            &CGPoint::new(0.0, 44.0),
            &CGSize::new(window_width, window_height - header_height - 44.0),
        );
        let drawer_scroll: Id = msg_send![ns_scroll_view, alloc];
        let drawer_scroll: Id = msg_send![drawer_scroll, initWithFrame: drawer_frame];
        let _: () = msg_send![drawer_scroll, setHasVerticalScroller: true];
        let _: () = msg_send![drawer_scroll, setBorderType: 0];
        let _: () = msg_send![drawer_scroll, setDrawsBackground: false];

        let drawer_content_size: CGSize = msg_send![drawer_scroll, contentSize];
        let drawer_stack_frame = CGRect::new(&CGPoint::new(0.0, 0.0), &drawer_content_size);
        let drawer_container = create_vertical_stack_view(drawer_stack_frame);
        let _: () = msg_send![drawer_scroll, setDocumentView: drawer_container];
        add_subview(blur_view, drawer_scroll);

        // Agent scroll view
        let agent_scroll: Id = msg_send![ns_scroll_view, alloc];
        let agent_scroll: Id = msg_send![agent_scroll, initWithFrame: drawer_frame];
        let _: () = msg_send![agent_scroll, setHasVerticalScroller: true];
        let _: () = msg_send![agent_scroll, setBorderType: 0];
        let _: () = msg_send![agent_scroll, setDrawsBackground: false];

        let agent_content_size: CGSize = msg_send![agent_scroll, contentSize];
        let agent_stack_frame = CGRect::new(&CGPoint::new(0.0, 0.0), &agent_content_size);
        let agent_container = create_vertical_stack_view(agent_stack_frame);
        let _: () = msg_send![agent_scroll, setDocumentView: agent_container];
        add_subview(blur_view, agent_scroll);

        // Footer/Search
        let search_frame = CGRect::new(
            &CGPoint::new(16.0, 10.0),
            &CGSize::new(window_width - 32.0, 24.0),
        );
        let search_field: Id = msg_send![ns_search_field, alloc];
        let search_field: Id = msg_send![search_field, initWithFrame: search_frame];
        add_subview(blur_view, search_field);

        // Agent input bar
        let input_frame = CGRect::new(
            &CGPoint::new(16.0, 10.0),
            &CGSize::new(window_width - 60.0, 32.0),
        );
        let agent_input: Id = msg_send![ns_text_field, alloc];
        let agent_input: Id = msg_send![agent_input, initWithFrame: input_frame];
        let _: () = msg_send![agent_input, setPlaceholderString: ns_string("Napisz polecenie...")];
        add_subview(blur_view, agent_input);

        let send_frame = CGRect::new(
            &CGPoint::new(window_width - 36.0, 10.0),
            &CGSize::new(24.0, 32.0),
        );
        let send_button = create_button(send_frame, ">", button_style::INLINE);
        add_subview(blur_view, send_button);

        // Wire handlers
        let handler_class = action_handler_class();
        let action_handler: Id = msg_send![handler_class, new];
        button_set_action(close_button, action_handler, sel!(onClose:));
        button_set_action(send_button, action_handler, sel!(onSend:));
        let _: () = msg_send![agent_input, setTarget: action_handler];
        let _: () = msg_send![agent_input, setAction: sel!(onInputSubmit:)];
        let _: () = msg_send![tab_control, setTarget: action_handler];
        let _: () = msg_send![tab_control, setAction: sel!(onTabChanged:)];
        let _: () = msg_send![search_field, setTarget: action_handler];
        let _: () = msg_send![search_field, setAction: sel!(onSearchChanged:)];

        // Store state
        state.window = Some(window as usize);
        state.window_delegate = Some(window_delegate as usize);
        state.blur_view = Some(blur_view as usize);
        state.title_label = Some(title_label as usize);
        state.tab_control = Some(tab_control as usize);
        state.close_button = Some(close_button as usize);
        state.settings_button = Some(settings_button as usize);
        state.drawer_scroll_view = Some(drawer_scroll as usize);
        state.drawer_container = Some(drawer_container as usize);
        state.search_field = Some(search_field as usize);
        state.agent_scroll_view = Some(agent_scroll as usize);
        state.agent_container = Some(agent_container as usize);
        state.agent_input_field = Some(agent_input as usize);
        state.agent_send_button = Some(send_button as usize);
        state.action_handler = Some(action_handler as usize);

        refresh_drawer_impl();
        update_chat_view_with_state(&mut state, true);
        update_input_field_with_state(&mut state);
        update_send_button_with_state(&mut state);
        set_active_tab_impl(OverlayTab::Drawer);

        window_set_alpha(window, 0.0);
        window_show(window);
        crate::ui_helpers::animate_fade(window, 1.0, 0.2);

        // Hide agent-only controls when in drawer tab
        set_hidden(agent_scroll, true);
        set_hidden(agent_input, true);
        set_hidden(send_button, true);
    }
}
