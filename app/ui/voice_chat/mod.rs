//! Voice Chat UI overlay for displaying streaming responses.
//!
//! This module provides a floating overlay window with:
//! - Drawer tab: clipboard-style transcription cards
//! - Agent tab: chat bubbles with streaming LLM responses

mod api;
mod handlers;
mod state;

// Re-export public API
pub use api::{
    add_voice_chat_error_message, add_voice_chat_user_message, append_voice_chat_assistant_delta,
    append_voice_chat_user_delta, clear_voice_chat_text, filter_drawer, hide_voice_chat_overlay,
    is_auto_send_enabled, is_conversation_active, is_voice_chat_overlay_visible, refresh_drawer,
    reset_voice_chat_activity, send_voice_chat_draft, set_voice_chat_send_callback,
    set_voice_chat_sending, set_voice_chat_target_app, set_voice_chat_text,
    set_voice_chat_user_text, show_agent_tab, show_drawer_tab, update_conversation_state,
    update_drawer_after_save, update_voice_chat_context_summary, update_voice_chat_status,
};
pub use state::{ConversationModeState, VoiceChatOverlayConfig};

use codescribe_core::config::{Config, OverlayPositionMode};
use core_graphics::geometry::{CGPoint, CGRect, CGSize};
use dispatch::Queue;
use objc::runtime::{Class, Object};
use objc::{msg_send, sel, sel_impl};
use objc2_app_kit::{
    NSBackingStoreType, NSVisualEffectBlendingMode, NSVisualEffectMaterial, NSVisualEffectState,
    NSWindowCollectionBehavior, NSWindowStyleMask,
};
use tracing::{info, warn};

use crate::ui_helpers::{
    NS_FLOATING_WINDOW_LEVEL, add_subview, button_set_action, button_style, color_clear,
    create_button, create_segmented_control, create_vertical_stack_view, ns_string, set_hidden,
    set_tooltip, window_set_alpha, window_show,
};

use api::update_active_tab_impl;
use handlers::{action_handler_class, overlay_window_class, window_delegate_class};
use state::{OVERLAY_STATE, Tab};

// Type alias for Objective-C object pointers
pub type Id = *mut Object;

/// Show the voice chat overlay window
pub fn show_voice_chat_overlay() {
    Queue::main().exec_async(|| {
        show_voice_chat_overlay_impl();
    });
}

/// Show the voice chat overlay with custom configuration
pub fn show_voice_chat_overlay_with_config(_config: VoiceChatOverlayConfig) {
    Queue::main().exec_async(|| {
        show_voice_chat_overlay_impl();
    });
}

fn show_voice_chat_overlay_impl() {
    unsafe {
        let mut state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());

        let ns_window = Class::get("NSWindow").unwrap();
        let ns_screen = Class::get("NSScreen").unwrap();

        if let Some(window_ptr) = state.window {
            let window = window_ptr as Id;
            let is_window: bool = msg_send![window, isKindOfClass: ns_window];
            if is_window {
                // Ensure previously-created overlays remain visible and sized correctly.
                let _: () = msg_send![window, orderFrontRegardless];
                let _: () = msg_send![window, setAlphaValue: 1.0f64];

                if let Some(blur_ptr) = state.blur_view {
                    let blur_view = blur_ptr as Id;
                    let w_frame: CGRect = msg_send![window, frame];
                    let blur_frame = CGRect::new(
                        &CGPoint::new(0.0, 0.0),
                        &CGSize::new(w_frame.size.width, w_frame.size.height),
                    );
                    let _: () = msg_send![blur_view, setFrame: blur_frame];
                }

                info!("Voice chat overlay reused");
                return;
            }
            warn!("Voice chat overlay pointer invalid; recreating window");
            api::clear_overlay_state(&mut state);
        }

        let config = VoiceChatOverlayConfig::default();
        let window_width = config.width;
        let window_height = config.height;
        let margin = 20.0;

        let main_screen: Id = msg_send![ns_screen, mainScreen];
        let visible_frame: CGRect = msg_send![main_screen, visibleFrame];

        let (raw_x, raw_y) = match Config::load().overlay_position_mode {
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
                let config = Config::load();
                (
                    config.overlay_custom_x.unwrap_or(def_x),
                    config.overlay_custom_y.unwrap_or(def_y),
                )
            }
        };

        let (x, y) = crate::ui_helpers::clamp_overlay_position(
            visible_frame,
            window_width,
            window_height,
            margin,
            raw_x,
            raw_y,
        );

        info!(
            "Voice chat overlay frame x={:.1} y={:.1} w={:.1} h={:.1}",
            x, y, window_width, window_height
        );

        let frame = CGRect {
            origin: CGPoint { x, y },
            size: CGSize {
                width: window_width,
                height: window_height,
            },
        };

        let overlay_window_class = overlay_window_class();
        let window: Id = msg_send![overlay_window_class, alloc];
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
        let _: () = msg_send![window, setLevel: NS_FLOATING_WINDOW_LEVEL];
        // Make sure the overlay shows up even when the user is in a fullscreen Space.
        let collection_behavior = NSWindowCollectionBehavior::CanJoinAllSpaces
            | NSWindowCollectionBehavior::FullScreenAuxiliary;
        let _: () = msg_send![window, setCollectionBehavior: collection_behavior];

        let delegate_class = window_delegate_class();
        let window_delegate: Id = msg_send![delegate_class, new];
        let _: () = msg_send![window, setDelegate: window_delegate];

        let content_view: Id = msg_send![window, contentView];

        let ns_visual = Class::get("NSVisualEffectView").unwrap();
        let blur_view: Id = msg_send![ns_visual, alloc];
        let blur_frame = CGRect::new(
            &CGPoint::new(0.0, 0.0),
            &CGSize::new(window_width, window_height),
        );
        let blur_view: Id = msg_send![blur_view, initWithFrame: blur_frame];
        let _: () = msg_send![blur_view, setMaterial: NSVisualEffectMaterial::HUDWindow];
        let _: () = msg_send![blur_view, setBlendingMode: NSVisualEffectBlendingMode::BehindWindow];
        let _: () = msg_send![blur_view, setState: NSVisualEffectState::Active];
        let _: () = msg_send![blur_view, setWantsLayer: true];
        let layer: Id = msg_send![blur_view, layer];
        if !layer.is_null() {
            let _: () = msg_send![layer, setCornerRadius: 16.0f64];
            let _: () = msg_send![layer, setMasksToBounds: true];
        }
        add_subview(content_view, blur_view);

        let action_handler_class = action_handler_class();
        let action_handler: Id = msg_send![action_handler_class, new];

        let header_height = 44.0;
        let footer_height = 44.0;
        let agent_input_height = 52.0;

        // Header
        let header_frame = CGRect::new(
            &CGPoint::new(0.0, window_height - header_height),
            &CGSize::new(window_width, header_height),
        );
        let header_view: Id = msg_send![Class::get("NSView").unwrap(), alloc];
        let header_view: Id = msg_send![header_view, initWithFrame: header_frame];
        let _: () = msg_send![header_view, setWantsLayer: true];
        let header_layer: Id = msg_send![header_view, layer];
        if !header_layer.is_null() {
            let color: Id = msg_send![Class::get("NSColor").unwrap(), colorWithRed: 0.15 green: 0.15 blue: 0.15 alpha: 0.6];
            let cg_color: Id = msg_send![color, CGColor];
            let _: () = msg_send![header_layer, setBackgroundColor: cg_color];
        }
        add_subview(blur_view, header_view);

        let title_label = crate::ui_helpers::create_label(crate::ui_helpers::LabelConfig {
            frame: CGRect::new(
                &CGPoint::new(16.0, window_height - 30.0),
                &CGSize::new(160.0, 20.0),
            ),
            text: "CodeScribe".to_string(),
            font_size: 13.0,
            bold: true,
            text_color: crate::ui_helpers::color_white(0.9),
            background_color: None,
            selectable: false,
            editable: false,
        });
        add_subview(blur_view, title_label);

        let tab_control = create_segmented_control(
            CGRect::new(
                &CGPoint::new(170.0, window_height - 34.0),
                &CGSize::new(120.0, 24.0),
            ),
            &["Drawer", "Agent"],
        );
        button_set_action(tab_control, action_handler, sel!(onTabChanged:));
        set_tooltip(
            tab_control,
            "Przełącz widok: Drawer (historia) / Agent (czat)",
        );
        add_subview(blur_view, tab_control);

        let paste_last_button = create_button(
            CGRect::new(
                &CGPoint::new(window_width - 160.0, window_height - 34.0),
                &CGSize::new(24.0, 24.0),
            ),
            "⇲",
            button_style::SMALL_SQUARE,
        );
        button_set_action(
            paste_last_button,
            action_handler,
            sel!(onPasteLastResponse:),
        );
        set_tooltip(paste_last_button, "Wklej ostatnią odpowiedź AI");
        add_subview(blur_view, paste_last_button);

        let copy_last_button = create_button(
            CGRect::new(
                &CGPoint::new(window_width - 128.0, window_height - 34.0),
                &CGSize::new(24.0, 24.0),
            ),
            "⧉",
            button_style::SMALL_SQUARE,
        );
        button_set_action(copy_last_button, action_handler, sel!(onCopyLastResponse:));
        set_tooltip(copy_last_button, "Skopiuj ostatnią odpowiedź AI");
        add_subview(blur_view, copy_last_button);

        let new_thread_button = create_button(
            CGRect::new(
                &CGPoint::new(window_width - 96.0, window_height - 34.0),
                &CGSize::new(28.0, 24.0),
            ),
            "↻",
            button_style::SMALL_SQUARE,
        );
        button_set_action(new_thread_button, action_handler, sel!(onNewThread:));
        set_tooltip(new_thread_button, "Nowy wątek (wyczyść czat)");
        add_subview(blur_view, new_thread_button);

        let close_button = create_button(
            CGRect::new(
                &CGPoint::new(window_width - 64.0, window_height - 34.0),
                &CGSize::new(24.0, 24.0),
            ),
            "✕",
            button_style::SMALL_SQUARE,
        );
        button_set_action(close_button, action_handler, sel!(onClose:));
        set_tooltip(close_button, "Zamknij okno");
        add_subview(blur_view, close_button);

        // Drawer scroll view + stack
        let drawer_frame = CGRect::new(
            &CGPoint::new(16.0, footer_height + 10.0),
            &CGSize::new(
                window_width - 32.0,
                window_height - header_height - footer_height - 20.0,
            ),
        );
        let ns_scroll = Class::get("NSScrollView").unwrap();
        let drawer_scroll: Id = msg_send![ns_scroll, alloc];
        let drawer_scroll: Id = msg_send![drawer_scroll, initWithFrame: drawer_frame];
        let _: () = msg_send![drawer_scroll, setHasVerticalScroller: true];
        let _: () = msg_send![drawer_scroll, setDrawsBackground: false];

        let drawer_container = create_vertical_stack_view(CGRect::new(
            &CGPoint::new(0.0, 0.0),
            &CGSize::new(drawer_frame.size.width, drawer_frame.size.height),
        ));
        let _: () = msg_send![drawer_scroll, setDocumentView: drawer_container];
        add_subview(blur_view, drawer_scroll);

        // Agent scroll view + stack
        let agent_scroll: Id = msg_send![ns_scroll, alloc];
        let agent_scroll: Id = msg_send![agent_scroll, initWithFrame: drawer_frame];
        let _: () = msg_send![agent_scroll, setHasVerticalScroller: true];
        let _: () = msg_send![agent_scroll, setDrawsBackground: false];
        let agent_container = create_vertical_stack_view(CGRect::new(
            &CGPoint::new(0.0, 0.0),
            &CGSize::new(drawer_frame.size.width, drawer_frame.size.height),
        ));
        let _: () = msg_send![agent_scroll, setDocumentView: agent_container];
        add_subview(blur_view, agent_scroll);

        // Drawer footer (search + dropdowns)
        let ns_search = Class::get("NSSearchField").unwrap();
        let search_field: Id = msg_send![ns_search, alloc];
        let search_frame = CGRect::new(
            &CGPoint::new(16.0, 12.0),
            &CGSize::new(window_width - 32.0, 24.0),
        );
        let search_field: Id = msg_send![search_field, initWithFrame: search_frame];
        let placeholder = ns_string("Search...");
        let _: () = msg_send![search_field, setPlaceholderString: placeholder];
        let _: () = msg_send![search_field, setTarget: action_handler];
        let _: () = msg_send![search_field, setAction: sel!(onSearchChanged:)];
        add_subview(blur_view, search_field);

        // Agent input bar
        let input_bar: Id = msg_send![Class::get("NSView").unwrap(), alloc];
        let input_frame = CGRect::new(
            &CGPoint::new(16.0, 8.0),
            &CGSize::new(window_width - 32.0, agent_input_height),
        );
        let input_bar: Id = msg_send![input_bar, initWithFrame: input_frame];
        let _: () = msg_send![input_bar, setWantsLayer: true];
        let input_layer: Id = msg_send![input_bar, layer];
        if !input_layer.is_null() {
            let color: Id = msg_send![Class::get("NSColor").unwrap(), colorWithRed: 0.15 green: 0.15 blue: 0.15 alpha: 0.6];
            let cg_color: Id = msg_send![color, CGColor];
            let _: () = msg_send![input_layer, setBackgroundColor: cg_color];
            let _: () = msg_send![input_layer, setCornerRadius: 18.0f64];
        }
        add_subview(blur_view, input_bar);

        let ns_text_field = Class::get("NSTextField").unwrap();
        let agent_input: Id = msg_send![ns_text_field, alloc];
        let agent_input: Id = msg_send![agent_input, initWithFrame: CGRect::new(&CGPoint::new(12.0, 12.0), &CGSize::new(window_width - 90.0, 28.0))];
        let _: () = msg_send![agent_input, setBezeled: true];
        let _: () = msg_send![agent_input, setPlaceholderString: ns_string("Napisz polecenie...")];
        let _: () = msg_send![agent_input, setTarget: action_handler];
        let _: () = msg_send![agent_input, setAction: sel!(onInputSubmit:)];
        let _: () = msg_send![input_bar, addSubview: agent_input];

        let agent_send_button = create_button(
            CGRect::new(
                &CGPoint::new(window_width - 76.0, 10.0),
                &CGSize::new(36.0, 32.0),
            ),
            ">",
            button_style::ROUNDED,
        );
        button_set_action(agent_send_button, action_handler, sel!(onSend:));
        let _: () = msg_send![input_bar, addSubview: agent_send_button];

        // Initial visibility
        set_hidden(agent_scroll, true);
        set_hidden(input_bar, true);

        state.window = Some(window as usize);
        state.window_delegate = Some(window_delegate as usize);
        state.blur_view = Some(blur_view as usize);
        state.title_label = Some(title_label as usize);
        state.tab_control = Some(tab_control as usize);
        state.close_button = Some(close_button as usize);
        state.settings_button = None;
        state.drawer_scroll_view = Some(drawer_scroll as usize);
        state.drawer_container = Some(drawer_container as usize);
        state.search_field = Some(search_field as usize);
        state.agent_scroll_view = Some(agent_scroll as usize);
        state.agent_container = Some(agent_container as usize);
        state.agent_input_field = Some(agent_input as usize);
        state.agent_send_button = Some(agent_send_button as usize);
        state.action_handler = Some(action_handler as usize);
        state.active_tab = Tab::Drawer;

        window_set_alpha(window, 0.0);
        window_show(window);
        crate::ui_helpers::animate_fade(window, 1.0, 0.2);

        let has_messages = !state.messages.is_empty();
        let desired_tab = if has_messages {
            Tab::Agent
        } else {
            state.active_tab
        };
        drop(state);
        api::refresh_drawer();
        update_active_tab_impl(desired_tab);
        if has_messages || matches!(desired_tab, Tab::Agent) {
            let mut state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
            api::update_chat_view_with_state(&mut state, true);
        }
    }
}
