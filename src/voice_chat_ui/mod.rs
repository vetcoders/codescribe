//! Voice Chat UI overlay for displaying streaming responses.
//!
//! Drawer-first UI with transcription cards and Agent chat tab.

mod api;
mod handlers;
mod state;

// Re-export public API
pub use api::{
    add_voice_chat_error_message, add_voice_chat_user_message, append_voice_chat_assistant_delta,
    hide_voice_chat_overlay, is_voice_chat_overlay_visible, refresh_drawer, set_active_tab,
    set_voice_chat_send_callback, set_voice_chat_sending, set_voice_chat_text, show_agent_tab,
    update_voice_chat_status,
};

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
    add_subview, button_set_action, button_style, create_button, create_segmented_control,
    create_vertical_stack_view, ns_string, set_hidden,
};

use api::{refresh_drawer, update_chat_view_with_state, update_input_field_with_state};
use handlers::{action_handler_class, window_delegate_class};
use state::{Tab, OVERLAY_STATE};

// Type alias for Objective-C object pointers
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

        if let Some(window_ptr) = state.window {
            let window = window_ptr as Id;
            let _: () = msg_send![window, orderFrontRegardless];
            info!("Voice chat overlay reused");
            return;
        }

        state.is_sending = false;

        let ns_window = Class::get("NSWindow").unwrap();
        let ns_text_field = Class::get("NSTextField").unwrap();
        let ns_color = Class::get("NSColor").unwrap();
        let ns_visual_effect_view = Class::get("NSVisualEffectView").unwrap();
        let ns_scroll_view = Class::get("NSScrollView").unwrap();

        let ns_screen = Class::get("NSScreen").unwrap();
        let main_screen: Id = msg_send![ns_screen, mainScreen];
        let visible_frame: CGRect = msg_send![main_screen, visibleFrame];

        let window_width = 450.0;
        let window_height = 520.0;
        let margin = 20.0;

        let right_x = visible_frame.origin.x + visible_frame.size.width;
        let top_y = visible_frame.origin.y + visible_frame.size.height;
        let x = right_x - window_width - margin;
        let y = top_y - window_height - margin;

        let frame = CGRect {
            origin: CGPoint { x, y },
            size: CGSize {
                width: window_width,
                height: window_height,
            },
        };

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

        let _: () = msg_send![window, setOpaque: false];
        let clear_color: Id = msg_send![ns_color, clearColor];
        let _: () = msg_send![window, setBackgroundColor: clear_color];
        let _: () = msg_send![window, setLevel: NS_FLOATING_WINDOW_LEVEL];
        let _: () = msg_send![window, setMovableByWindowBackground: true];
        let _: () = msg_send![window, setHasShadow: true];
        let collection_behavior = NSWindowCollectionBehavior::CanJoinAllSpaces;
        let _: () = msg_send![window, setCollectionBehavior: collection_behavior];
        let delegate_class = window_delegate_class();
        let window_delegate: Id = msg_send![delegate_class, new];
        let _: () = msg_send![window, setDelegate: window_delegate];

        let content_view: Id = msg_send![window, contentView];

        // Background blur
        let blur_frame = CGRect {
            origin: CGPoint { x: 0.0, y: 0.0 },
            size: CGSize {
                width: window_width,
                height: window_height,
            },
        };
        let blur_view: Id = msg_send![ns_visual_effect_view, alloc];
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

        // Header
        let header_height = 44.0;
        let header_frame = CGRect {
            origin: CGPoint {
                x: 0.0,
                y: window_height - header_height,
            },
            size: CGSize {
                width: window_width,
                height: header_height,
            },
        };
        let header_view: Id = msg_send![Class::get("NSView").unwrap(), alloc];
        let header_view: Id = msg_send![header_view, initWithFrame: header_frame];
        add_subview(content_view, header_view);

        let title_frame = CGRect {
            origin: CGPoint { x: 16.0, y: 12.0 },
            size: CGSize {
                width: 140.0,
                height: 20.0,
            },
        };
        let title_label: Id = msg_send![ns_text_field, alloc];
        let title_label: Id = msg_send![title_label, initWithFrame: title_frame];
        let _: () = msg_send![title_label, setBezeled: false];
        let _: () = msg_send![title_label, setDrawsBackground: false];
        let _: () = msg_send![title_label, setEditable: false];
        let _: () = msg_send![title_label, setSelectable: false];
        let _: () = msg_send![title_label, setStringValue: ns_string("CodeScribe")];
        let white: Id = msg_send![ns_color, whiteColor];
        let _: () = msg_send![title_label, setTextColor: white];
        add_subview(header_view, title_label);

        let tab_frame = CGRect {
            origin: CGPoint { x: 170.0, y: 8.0 },
            size: CGSize {
                width: 140.0,
                height: 28.0,
            },
        };
        let tab_control = create_segmented_control(tab_frame, &["Drawer", "Agent"]);

        let action_handler_class = action_handler_class();
        let action_handler: Id = msg_send![action_handler_class, new];
        let _: () = msg_send![tab_control, setTarget: action_handler];
        let _: () = msg_send![tab_control, setAction: sel!(onTabChanged:)];
        let _: () = msg_send![tab_control, setSelectedSegment: 0_isize];
        add_subview(header_view, tab_control);

        let settings_button = create_button(
            CGRect::new(&CGPoint::new(window_width - 64.0, 8.0), &CGSize::new(24.0, 24.0)),
            "⚙",
            button_style::INLINE,
        );
        add_subview(header_view, settings_button);

        let close_button = create_button(
            CGRect::new(&CGPoint::new(window_width - 32.0, 8.0), &CGSize::new(24.0, 24.0)),
            "✕",
            button_style::INLINE,
        );
        button_set_action(close_button, action_handler, sel!(onClose:));
        add_subview(header_view, close_button);

        // Drawer scroll view
        let footer_height = 44.0;
        let drawer_frame = CGRect {
            origin: CGPoint {
                x: 16.0,
                y: footer_height + 8.0,
            },
            size: CGSize {
                width: window_width - 32.0,
                height: window_height - header_height - footer_height - 16.0,
            },
        };
        let drawer_scroll: Id = msg_send![ns_scroll_view, alloc];
        let drawer_scroll: Id = msg_send![drawer_scroll, initWithFrame: drawer_frame];
        let _: () = msg_send![drawer_scroll, setHasVerticalScroller: true];
        let _: () = msg_send![drawer_scroll, setDrawsBackground: false];

        let drawer_stack = create_vertical_stack_view(drawer_frame);
        let _: () = msg_send![drawer_scroll, setDocumentView: drawer_stack];
        add_subview(content_view, drawer_scroll);

        // Agent scroll view
        let agent_frame = drawer_frame;
        let agent_scroll: Id = msg_send![ns_scroll_view, alloc];
        let agent_scroll: Id = msg_send![agent_scroll, initWithFrame: agent_frame];
        let _: () = msg_send![agent_scroll, setHasVerticalScroller: true];
        let _: () = msg_send![agent_scroll, setDrawsBackground: false];

        let agent_stack = create_vertical_stack_view(agent_frame);
        let _: () = msg_send![agent_scroll, setDocumentView: agent_stack];
        add_subview(content_view, agent_scroll);

        // Footer (search for drawer)
        let search_frame = CGRect {
            origin: CGPoint { x: 16.0, y: 10.0 },
            size: CGSize {
                width: window_width - 32.0,
                height: 24.0,
            },
        };
        let search_field: Id = msg_send![ns_text_field, alloc];
        let search_field: Id = msg_send![search_field, initWithFrame: search_frame];
        let _: () = msg_send![search_field, setPlaceholderString: ns_string("Search...")];
        let _: () = msg_send![search_field, setBezeled: true];
        let _: () = msg_send![search_field, setEditable: true];
        let _: () = msg_send![search_field, setSelectable: true];
        let _: () = msg_send![search_field, setTarget: action_handler];
        let _: () = msg_send![search_field, setAction: sel!(onSearchChanged:)];
        add_subview(content_view, search_field);

        // Agent input
        let input_frame = CGRect {
            origin: CGPoint { x: 16.0, y: 6.0 },
            size: CGSize {
                width: window_width - 64.0,
                height: 32.0,
            },
        };
        let input_field: Id = msg_send![ns_text_field, alloc];
        let input_field: Id = msg_send![input_field, initWithFrame: input_frame];
        let _: () = msg_send![input_field, setPlaceholderString: ns_string("Napisz polecenie...")];
        let _: () = msg_send![input_field, setBezeled: true];
        let _: () = msg_send![input_field, setEditable: true];
        let _: () = msg_send![input_field, setSelectable: true];
        let _: () = msg_send![input_field, setTarget: action_handler];
        let _: () = msg_send![input_field, setAction: sel!(onInputSubmit:)];
        add_subview(content_view, input_field);

        let send_button = create_button(
            CGRect::new(
                &CGPoint::new(window_width - 40.0, 6.0),
                &CGSize::new(24.0, 24.0),
            ),
            ">",
            button_style::INLINE,
        );
        button_set_action(send_button, action_handler, sel!(onSend:));
        add_subview(content_view, send_button);

        set_hidden(agent_scroll, true);
        set_hidden(input_field, true);
        set_hidden(send_button, true);

        // Update state
        state.window = Some(window as usize);
        state.window_delegate = Some(window_delegate as usize);
        state.blur_view = Some(blur_view as usize);
        state.title_label = Some(title_label as usize);
        state.tab_control = Some(tab_control as usize);
        state.close_button = Some(close_button as usize);
        state.settings_button = Some(settings_button as usize);
        state.drawer_scroll_view = Some(drawer_scroll as usize);
        state.drawer_container = Some(drawer_stack as usize);
        state.search_field = Some(search_field as usize);
        state.agent_scroll_view = Some(agent_scroll as usize);
        state.agent_container = Some(agent_stack as usize);
        state.agent_input_field = Some(input_field as usize);
        state.agent_send_button = Some(send_button as usize);
        state.action_handler = Some(action_handler as usize);
        state.active_tab = Tab::Drawer;

        refresh_drawer();
        update_chat_view_with_state(&mut state, true);
        update_input_field_with_state(&state);

        let _: () = msg_send![window, orderFrontRegardless];
        info!("Voice chat overlay shown (drawer + agent)");
    }
}
