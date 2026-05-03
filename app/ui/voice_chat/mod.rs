//! Voice Chat UI overlay for displaying streaming responses.
//!
//! This module provides a floating overlay window with:
//! - Drawer tab: clipboard-style transcription cards
//! - Agent tab: chat bubbles with streaming LLM responses
//! - Settings button: opens the persistent settings window

mod api;
mod handlers;
mod state;

// Re-export public API
pub use api::{
    add_voice_chat_error_message, add_voice_chat_system_message, add_voice_chat_user_message,
    append_voice_chat_assistant_delta, append_voice_chat_user_delta, clear_voice_chat_text,
    dispatch_voice_chat_send, filter_drawer, finalize_voice_chat_assistant_message,
    finalize_voice_chat_user_message, handoff_transcript_to_chat, hide_voice_chat_overlay,
    is_auto_send_enabled, is_conversation_active, is_voice_chat_overlay_visible, refresh_drawer,
    reset_voice_chat_activity, send_voice_chat_draft, set_voice_chat_runtime_degraded,
    set_voice_chat_send_callback, set_voice_chat_sending, set_voice_chat_target_app,
    set_voice_chat_text, set_voice_chat_user_text, show_agent_tab, show_drawer_tab,
    update_conversation_state, update_drawer_after_save, update_voice_chat_context_summary,
    update_voice_chat_status,
};
pub use state::{ConversationModeState, VoiceChatOverlayConfig};

use codescribe_core::config::{Config, OverlayPositionMode};
use core_graphics::geometry::{CGPoint, CGRect, CGSize};
use dispatch::Queue;
use objc::runtime::{Class, Object};
use objc::{msg_send, sel, sel_impl};
use objc2_app_kit::{NSVisualEffectBlendingMode, NSVisualEffectMaterial, NSVisualEffectState};
use std::thread;
use std::time::Duration;
use tracing::{debug, info, warn};

use crate::config::ShortcutBinding;
use crate::os::hotkeys::ModeHotkeyBindings;

use crate::ui_helpers::{
    LabelConfig, add_subview, agent_chat_shell_frame, agent_chat_shell_panel_policy,
    apply_shared_shell_panel_policy, apply_tafla_surface, button_set_action, button_style,
    chat_header_layout, chat_input_row_layout, color_clear, color_secondary_label, create_button,
    create_flipped_vertical_stack_view, create_glass_effect_view_with, create_label,
    create_scrollable_text_view, create_vertical_stack_view, layout_region_frame_for_view,
    main_screen_visible_frame, ns_string, present_shared_shell_panel, set_button_symbol,
    set_focus_ring, set_glass_effect_content_view, set_hidden, set_tooltip,
    style_toolbar_icon_button, ui_colors, ui_tokens, window_set_alpha,
};

use api::update_active_tab_impl;
use handlers::{
    action_handler_class, drop_target_view_class, overlay_window_class, window_delegate_class,
};
use state::{OVERLAY_STATE, Tab};

// Type alias for Objective-C object pointers
pub type Id = *mut Object;

// NSViewAutoresizingMaskOptions (bitmask)
const NSVIEW_MIN_X_MARGIN: isize = 1;
const NSVIEW_WIDTH_SIZABLE: isize = 2;
const NSVIEW_MAX_X_MARGIN: isize = 4;
const NSVIEW_MIN_Y_MARGIN: isize = 8;
const NSVIEW_HEIGHT_SIZABLE: isize = 16;
const NSVIEW_MAX_Y_MARGIN: isize = 32;

pub(super) fn shortcuts_lines(bindings: ModeHotkeyBindings) -> (String, String) {
    let hold_line = match bindings.dictation {
        ShortcutBinding::HoldFn => "Hold Fn — record • Fn+Shift — chat • Fn+Cmd — selection",
        ShortcutBinding::HoldCtrl => "Hold Ctrl — record",
        ShortcutBinding::HoldCtrlAlt => {
            "Hold Ctrl — record • Ctrl+Option — format • Ctrl+Shift — chat • Ctrl+Cmd — selection"
        }
        ShortcutBinding::HoldCtrlShift => "Hold Ctrl+Shift — record",
        ShortcutBinding::HoldCtrlCmd => "Hold Ctrl+Cmd — record",
        ShortcutBinding::Disabled
        | ShortcutBinding::DoubleCtrl
        | ShortcutBinding::DoubleLeftOption
        | ShortcutBinding::DoubleRightOption => "Hold-to-talk disabled",
    };

    let toggle_line = if bindings.dictation == ShortcutBinding::DoubleCtrl {
        "Ctrl Ctrl — toggle (raw)"
    } else {
        let formatting_left = bindings.formatting == ShortcutBinding::DoubleLeftOption;
        let assistive_right = bindings.assistive == ShortcutBinding::DoubleRightOption;
        match (formatting_left, assistive_right) {
            (true, true) => "⌥⌥ — toggle • Right ⌥⌥ — AI",
            (true, false) => "Left ⌥⌥ — toggle",
            (false, true) => "Right ⌥⌥ — AI",
            (false, false) => "Toggle disabled",
        }
    };

    (hold_line.to_string(), toggle_line.to_string())
}

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
        // DEADLOCK PREVENTION: Do NOT hold OVERLAY_STATE across AppKit calls.
        // AppKit can spin a nested runloop during window_show / animate_fade /
        // orderFront, and pending Queue::main().exec_async blocks that also lock
        // OVERLAY_STATE will deadlock on the non-reentrant Mutex.
        // (Same pattern documented in hide_voice_chat_overlay_impl.)

        // Phase 1 — check / reuse existing window (short lock scope).
        {
            let mut state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
            let ns_window = Class::get("NSWindow").unwrap();
            if let Some(window_ptr) = state.window {
                let window = window_ptr as Id;
                let is_window: bool = msg_send![window, isKindOfClass: ns_window];
                if is_window {
                    // Reuse path: extract pointers, release lock, THEN do AppKit.
                    let blur_ptr = state.blur_view;
                    drop(state);

                    if let Some(visible_frame) = main_screen_visible_frame() {
                        let shell_policy = agent_chat_shell_panel_policy(visible_frame);
                        apply_shared_shell_panel_policy(window, &shell_policy);
                    }
                    present_shared_shell_panel(window);
                    let _: () = msg_send![window, setAlphaValue: 1.0f64];

                    if let Some(blur_ptr) = blur_ptr {
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
        } // OVERLAY_STATE released — UI construction below is lock-free.

        // Phase 2 — build the entire overlay UI without holding OVERLAY_STATE.
        let config = VoiceChatOverlayConfig::default();
        let window_width = config.width;
        let window_height = config.height;
        let margin = 20.0;

        let Some(visible_frame) = main_screen_visible_frame() else {
            warn!("No NSScreen available for voice chat overlay");
            return;
        };

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

        let frame = agent_chat_shell_frame(
            visible_frame,
            window_width,
            window_height,
            margin,
            raw_x,
            raw_y,
        );

        info!(
            "Voice chat overlay frame x={:.1} y={:.1} w={:.1} h={:.1}",
            frame.origin.x, frame.origin.y, window_width, window_height
        );

        let shell_policy = agent_chat_shell_panel_policy(visible_frame);
        let overlay_window_class = overlay_window_class();
        let window: Id = msg_send![overlay_window_class, alloc];
        let window: Id = msg_send![
            window,
            initWithContentRect: frame
            styleMask: shell_policy.style_mask
            backing: shell_policy.backing_store
            defer: false
        ];

        let _: () = msg_send![window, setTitle: ns_string("CodeScribe")];
        apply_shared_shell_panel_policy(window, &shell_policy);

        let delegate_class = window_delegate_class();
        let window_delegate: Id = msg_send![delegate_class, new];
        let _: () = msg_send![window, setDelegate: window_delegate];

        let window_content_view: Id = msg_send![window, contentView];
        let ns_mut_array = Class::get("NSMutableArray").unwrap();
        let window_drag_types: Id = msg_send![ns_mut_array, array];
        let _: () = msg_send![window_drag_types, addObject: ns_string("public.file-url")];
        let _: () = msg_send![window_drag_types, addObject: ns_string("NSFilenamesPboardType")];
        let _: () = msg_send![window, registerForDraggedTypes: window_drag_types];

        let blur_frame = CGRect::new(
            &CGPoint::new(0.0, 0.0),
            &CGSize::new(window_width, window_height),
        );
        let blur_view: Id = create_glass_effect_view_with(
            blur_frame,
            NSVisualEffectMaterial::HUDWindow,
            NSVisualEffectBlendingMode::BehindWindow,
            NSVisualEffectState::Active,
        );
        let _: () = msg_send![
            blur_view,
            setAutoresizingMask: NSVIEW_WIDTH_SIZABLE | NSVIEW_HEIGHT_SIZABLE
        ];
        let layer: Id = msg_send![blur_view, layer];
        if !layer.is_null() {
            let bg = ui_colors::surface_glass();
            let cg_bg: Id = msg_send![bg, CGColor];
            let _: () = msg_send![layer, setBackgroundColor: cg_bg];
            apply_tafla_surface(layer, true);
            let _: () = msg_send![layer, setMasksToBounds: true];
        }
        add_subview(window_content_view, blur_view);
        let glass_content_view: Id = msg_send![Class::get("NSView").unwrap(), alloc];
        let glass_content_view: Id = msg_send![glass_content_view, initWithFrame: blur_frame];
        let _: () = msg_send![
            glass_content_view,
            setAutoresizingMask: NSVIEW_WIDTH_SIZABLE | NSVIEW_HEIGHT_SIZABLE
        ];
        let _: bool = set_glass_effect_content_view(blur_view, glass_content_view);
        let bounds: CGRect = msg_send![blur_view, bounds];
        let content_bounds = layout_region_frame_for_view(blur_view).unwrap_or(bounds);

        let action_handler_class = action_handler_class();
        let action_handler: Id = msg_send![action_handler_class, new];

        let content_pad = ui_tokens::EDGE_PADDING;

        let header_height = ui_tokens::HEADER_HEIGHT_COMPACT;
        let footer_height = ui_tokens::FOOTER_HEIGHT;
        // Start compact; grows dynamically as the user types/pastes more content.
        // Agent input starts compact and can grow with content (see `resize_agent_input_locked`).
        let agent_input_height = ui_tokens::AGENT_INPUT_HEIGHT;

        // Header container on the single root glass (no glass-on-glass dome).
        let header_frame = CGRect::new(
            &CGPoint::new(
                content_bounds.origin.x,
                content_bounds.origin.y + content_bounds.size.height - header_height,
            ),
            &CGSize::new(content_bounds.size.width.max(0.0), header_height),
        );
        let header_bg: Id = msg_send![Class::get("NSView").unwrap(), alloc];
        let header_bg: Id = msg_send![header_bg, initWithFrame: header_frame];
        let _: () = msg_send![header_bg, setWantsLayer: true];
        let _: () = msg_send![
            header_bg,
            setAutoresizingMask: NSVIEW_WIDTH_SIZABLE | NSVIEW_MIN_Y_MARGIN
        ];
        let header_layer: Id = msg_send![header_bg, layer];
        if !header_layer.is_null() {
            let clear_cg: Id = msg_send![color_clear(), CGColor];
            let _: () = msg_send![header_layer, setBackgroundColor: clear_cg];
        }
        let header_separator: Id = msg_send![Class::get("NSView").unwrap(), alloc];
        let header_separator: Id = msg_send![
            header_separator,
            initWithFrame: CGRect::new(
                &CGPoint::new(0.0, 0.0),
                &CGSize::new(header_frame.size.width.max(0.0), 1.0),
            )
        ];
        let _: () = msg_send![header_separator, setWantsLayer: true];
        let _: () = msg_send![
            header_separator,
            setAutoresizingMask: NSVIEW_WIDTH_SIZABLE | NSVIEW_MAX_Y_MARGIN
        ];
        let separator_layer: Id = msg_send![header_separator, layer];
        if !separator_layer.is_null() {
            let border = ui_colors::header_border();
            let cg_border: Id = msg_send![border, CGColor];
            let _: () = msg_send![separator_layer, setBackgroundColor: cg_border];
        }
        let header_controls: Id = msg_send![Class::get("NSView").unwrap(), alloc];
        let header_controls: Id = msg_send![
            header_controls,
            initWithFrame: CGRect::new(
                &CGPoint::new(0.0, 0.0),
                &CGSize::new(header_frame.size.width, header_frame.size.height),
            )
        ];
        let _: () = msg_send![header_controls, setWantsLayer: true];
        let _: () = msg_send![
            header_controls,
            setAutoresizingMask: NSVIEW_WIDTH_SIZABLE | NSVIEW_HEIGHT_SIZABLE
        ];
        // Header right-side controls (right-aligned, consistent spacing).
        let btn_w = ui_tokens::CHAT_HEADER_BUTTON_SIZE;
        let btn_h = ui_tokens::CHAT_HEADER_BUTTON_SIZE;
        let gap = ui_tokens::CHAT_HEADER_BUTTON_GAP;
        let right_pad = ui_tokens::EDGE_PADDING_TIGHT;
        let header_btn_y = ((header_height - btn_h) / 2.0).max(0.0);

        let mut x = header_frame.size.width - right_pad - btn_w;
        let more_button_x = x;
        x -= gap + btn_w;
        let help_button_x = x;
        x -= gap + btn_w;
        let favorites_button_x = x;
        x -= gap + btn_w;
        let record_button_x = x;

        // Keep the tab control outside the native traffic-light zone and before
        // the right-side icon cluster. The visible brand label lives in the
        // footer, because the native titlebar owns the top-left corner.
        let right_cluster_start_x = record_button_x;
        let header_safe_x = ui_tokens::TRAFFIC_LIGHTS_SPACER_WIDTH + 6.0;
        let header_layout = chat_header_layout(header_safe_x, 0.0, right_cluster_start_x);
        let tab_cluster_x = header_layout.tab_cluster_x;
        let tab_btn_w = header_layout.tab_button_width;
        let tab_gap = header_layout.tab_button_gap;
        let status_pill_x = header_layout.status_pill_x;
        let status_pill_w = header_layout.status_pill_width;

        let tab_drawer_button = create_button(
            CGRect::new(
                &CGPoint::new(tab_cluster_x, header_btn_y),
                &CGSize::new(tab_btn_w, btn_h),
            ),
            "",
            button_style::INLINE,
        );
        let _ = set_button_symbol(tab_drawer_button, "archivebox");
        style_toolbar_icon_button(tab_drawer_button);
        button_set_action(tab_drawer_button, action_handler, sel!(onTabDrawer:));
        set_tooltip(tab_drawer_button, "Drawer");
        let _: () = msg_send![
            tab_drawer_button,
            setAutoresizingMask: NSVIEW_MAX_X_MARGIN | NSVIEW_MIN_Y_MARGIN
        ];
        add_subview(header_controls, tab_drawer_button);

        let tab_agent_button = create_button(
            CGRect::new(
                &CGPoint::new(tab_cluster_x + (tab_btn_w + tab_gap) * 1.0, header_btn_y),
                &CGSize::new(tab_btn_w, btn_h),
            ),
            "",
            button_style::INLINE,
        );
        let _ = set_button_symbol(tab_agent_button, "bubble.left.and.bubble.right");
        style_toolbar_icon_button(tab_agent_button);
        button_set_action(tab_agent_button, action_handler, sel!(onTabAgent:));
        set_tooltip(tab_agent_button, "Agent");
        let _: () = msg_send![
            tab_agent_button,
            setAutoresizingMask: NSVIEW_MAX_X_MARGIN | NSVIEW_MIN_Y_MARGIN
        ];
        add_subview(header_controls, tab_agent_button);

        let tab_settings_button = create_button(
            CGRect::new(
                &CGPoint::new(tab_cluster_x + (tab_btn_w + tab_gap) * 2.0, header_btn_y),
                &CGSize::new(tab_btn_w, btn_h),
            ),
            "",
            button_style::INLINE,
        );
        let _ = set_button_symbol(tab_settings_button, "gearshape");
        style_toolbar_icon_button(tab_settings_button);
        button_set_action(tab_settings_button, action_handler, sel!(onTabSettings:));
        set_tooltip(tab_settings_button, "Settings");
        let _: () = msg_send![
            tab_settings_button,
            setAutoresizingMask: NSVIEW_MAX_X_MARGIN | NSVIEW_MIN_Y_MARGIN
        ];
        add_subview(header_controls, tab_settings_button);

        // Status pill (global status: Idle / Listening / Processing / Error).
        let status_pill_h = ui_tokens::STATUS_PILL_HEIGHT;
        let status_pill_y = ((header_height - status_pill_h) / 2.0).max(0.0);
        let status_pill_frame = CGRect::new(
            &CGPoint::new(status_pill_x, status_pill_y),
            &CGSize::new(status_pill_w, status_pill_h),
        );
        let status_pill: Id = msg_send![Class::get("NSView").unwrap(), alloc];
        let status_pill: Id = msg_send![status_pill, initWithFrame: status_pill_frame];
        let _: () = msg_send![status_pill, setWantsLayer: true];
        let status_layer: Id = msg_send![status_pill, layer];
        if !status_layer.is_null() {
            let bg = ui_colors::panel_bg();
            let cg_bg: Id = msg_send![bg, CGColor];
            let _: () = msg_send![status_layer, setBackgroundColor: cg_bg];
            apply_tafla_surface(status_layer, false);
            let border = ui_colors::header_border();
            let cg_border: Id = msg_send![border, CGColor];
            let _: () = msg_send![status_layer, setBorderColor: cg_border];
            let _: () = msg_send![status_layer, setBorderWidth: ui_tokens::SURFACE_BORDER_WIDTH];
            let _: () = msg_send![status_layer, setMasksToBounds: true];
        }
        let _: () = msg_send![
            status_pill,
            setAutoresizingMask: NSVIEW_MIN_X_MARGIN | NSVIEW_MIN_Y_MARGIN
        ];
        let _: () = msg_send![status_pill, setHidden: !header_layout.show_status_pill];

        let dot_size = ui_tokens::STATUS_DOT_SIZE;
        let dot: Id = msg_send![Class::get("NSView").unwrap(), alloc];
        let dot: Id = msg_send![
            dot,
            initWithFrame: CGRect::new(
                &CGPoint::new(
                    ui_tokens::STATUS_PILL_DOT_INSET_X,
                    (status_pill_h - dot_size) / 2.0,
                ),
                &CGSize::new(dot_size, dot_size),
            )
        ];
        let _: () = msg_send![dot, setWantsLayer: true];
        let dot_layer: Id = msg_send![dot, layer];
        if !dot_layer.is_null() {
            let _: () = msg_send![dot_layer, setCornerRadius: (dot_size / 2.0).max(2.0)];
            let _: () = msg_send![dot_layer, setMasksToBounds: true];
        }
        add_subview(status_pill, dot);

        let status_label = create_label(LabelConfig {
            frame: CGRect::new(
                &CGPoint::new(ui_tokens::STATUS_PILL_LABEL_INSET_X, 1.0),
                &CGSize::new(
                    (status_pill_w
                        - ui_tokens::STATUS_PILL_LABEL_INSET_X
                        - ui_tokens::STATUS_PILL_LABEL_INSET_RIGHT)
                        .max(0.0),
                    status_pill_h - 2.0,
                ),
            ),
            text: "Idle".to_string(),
            font_size: ui_tokens::MICRO_FONT_SIZE,
            bold: false,
            text_color: ui_colors::bubble_meta_text(),
            background_color: None,
            selectable: false,
            editable: false,
        });
        let _: () = msg_send![
            status_label,
            setAutoresizingMask: NSVIEW_WIDTH_SIZABLE | NSVIEW_MIN_Y_MARGIN
        ];
        add_subview(status_pill, status_label);
        add_subview(header_controls, status_pill);

        let record_button = create_button(
            CGRect::new(
                &CGPoint::new(record_button_x, header_btn_y),
                &CGSize::new(btn_w, btn_h),
            ),
            "",
            button_style::INLINE,
        );
        let has_symbol = set_button_symbol(record_button, "mic.fill");
        if !has_symbol {
            let _: () = msg_send![record_button, setTitle: ns_string("Rec")];
        }
        style_toolbar_icon_button(record_button);
        button_set_action(record_button, action_handler, sel!(onHeaderRecord:));
        set_tooltip(record_button, "Start/stop recording");
        let _: () = msg_send![
            record_button,
            setAutoresizingMask: NSVIEW_MIN_X_MARGIN | NSVIEW_MIN_Y_MARGIN
        ];
        add_subview(header_controls, record_button);

        // Drawer favorites filter (hearts on/off)
        let favorites_button = create_button(
            CGRect::new(
                &CGPoint::new(favorites_button_x, header_btn_y),
                &CGSize::new(btn_w, btn_h),
            ),
            "",
            button_style::INLINE,
        );
        let _ = set_button_symbol(favorites_button, "heart.circle");
        style_toolbar_icon_button(favorites_button);
        button_set_action(
            favorites_button,
            action_handler,
            sel!(onToggleFavoritesOnly:),
        );
        set_tooltip(favorites_button, "Show favorites only");
        let _: () = msg_send![
            favorites_button,
            setAutoresizingMask: NSVIEW_MIN_X_MARGIN | NSVIEW_MIN_Y_MARGIN
        ];
        add_subview(header_controls, favorites_button);

        let help_button = create_button(
            CGRect::new(
                &CGPoint::new(help_button_x, header_btn_y),
                &CGSize::new(btn_w, btn_h),
            ),
            "",
            button_style::INLINE,
        );
        let has_symbol = set_button_symbol(help_button, "questionmark.circle");
        if !has_symbol {
            let _: () = msg_send![help_button, setTitle: ns_string("?")];
        }
        style_toolbar_icon_button(help_button);
        button_set_action(help_button, action_handler, sel!(onShowShortcuts:));
        set_tooltip(help_button, "Keyboard shortcuts");
        let _: () = msg_send![
            help_button,
            setAutoresizingMask: NSVIEW_MIN_X_MARGIN | NSVIEW_MIN_Y_MARGIN
        ];
        add_subview(header_controls, help_button);

        let more_button = create_button(
            CGRect::new(
                &CGPoint::new(more_button_x, header_btn_y),
                &CGSize::new(btn_w, btn_h),
            ),
            "",
            button_style::INLINE,
        );
        let has_symbol = set_button_symbol(more_button, "ellipsis.circle");
        if !has_symbol {
            let _: () = msg_send![more_button, setTitle: ns_string("More")];
        }
        style_toolbar_icon_button(more_button);
        button_set_action(more_button, action_handler, sel!(onMoreMenu:));
        set_tooltip(more_button, "More actions");
        let _: () = msg_send![
            more_button,
            setAutoresizingMask: NSVIEW_MIN_X_MARGIN | NSVIEW_MIN_Y_MARGIN
        ];
        add_subview(header_controls, more_button);

        let close_button = create_button(
            CGRect::new(
                &CGPoint::new(header_frame.size.width + btn_w, header_btn_y),
                &CGSize::new(btn_w, btn_h),
            ),
            "",
            button_style::INLINE,
        );
        let has_symbol = set_button_symbol(close_button, "xmark");
        if !has_symbol {
            let _: () = msg_send![close_button, setTitle: ns_string("Close")];
        }
        style_toolbar_icon_button(close_button);
        button_set_action(close_button, action_handler, sel!(onClose:));
        set_tooltip(close_button, "Close window");
        let _: () = msg_send![close_button, setHidden: true];
        let _: () = msg_send![
            close_button,
            setAutoresizingMask: NSVIEW_MIN_X_MARGIN | NSVIEW_MIN_Y_MARGIN
        ];
        add_subview(header_controls, close_button);

        // Drawer/Agent split view on top of the single root glass.
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

        let ns_scroll = Class::get("NSScrollView").unwrap();
        let ns_view = Class::get("NSView").unwrap();
        let ns_view_controller = Class::get("NSViewController").unwrap();
        let split_cls = Class::get("NSSplitViewController").unwrap();
        let split_item_cls = Class::get("NSSplitViewItem").unwrap();

        let split_controller: Id = msg_send![split_cls, alloc];
        let split_controller: Id = msg_send![split_controller, init];

        let sidebar_controller: Id = msg_send![ns_view_controller, alloc];
        let sidebar_controller: Id = msg_send![sidebar_controller, init];
        let sidebar_frame = CGRect::new(
            &CGPoint::new(0.0, 0.0),
            &CGSize::new(content_frame.size.width, content_frame.size.height),
        );
        let sidebar_view: Id = msg_send![ns_view, alloc];
        let sidebar_view: Id = msg_send![sidebar_view, initWithFrame: sidebar_frame];
        let _: () = msg_send![sidebar_view, setWantsLayer: true];
        let _: () = msg_send![
            sidebar_view,
            setAutoresizingMask: NSVIEW_WIDTH_SIZABLE | NSVIEW_HEIGHT_SIZABLE
        ];
        let sidebar_layer: Id = msg_send![sidebar_view, layer];
        if !sidebar_layer.is_null() {
            let sidebar_bg = ui_colors::sidebar_bg();
            let cg_sidebar_bg: Id = msg_send![sidebar_bg, CGColor];
            let _: () = msg_send![sidebar_layer, setBackgroundColor: cg_sidebar_bg];
        }
        let _: () = msg_send![sidebar_controller, setView: sidebar_view];

        let content_controller: Id = msg_send![ns_view_controller, alloc];
        let content_controller: Id = msg_send![content_controller, init];
        let content_view: Id = msg_send![ns_view, alloc];
        let content_view: Id = msg_send![
            content_view,
            initWithFrame: CGRect::new(
                &CGPoint::new(0.0, 0.0),
                &CGSize::new(content_frame.size.width, content_frame.size.height),
            )
        ];
        let _: () = msg_send![content_view, setWantsLayer: true];
        let content_layer: Id = msg_send![content_view, layer];
        if !content_layer.is_null() {
            let content_bg = ui_colors::surface_glass();
            let cg_content_bg: Id = msg_send![content_bg, CGColor];
            let _: () = msg_send![content_layer, setBackgroundColor: cg_content_bg];
        }
        let _: () = msg_send![content_controller, setView: content_view];

        let has_sidebar_ctor: bool =
            msg_send![split_item_cls, respondsToSelector: sel!(sidebarWithViewController:)];
        let sidebar_item: Id = if has_sidebar_ctor {
            msg_send![split_item_cls, sidebarWithViewController: sidebar_controller]
        } else {
            msg_send![split_item_cls, splitViewItemWithViewController: sidebar_controller]
        };
        let content_item: Id =
            msg_send![split_item_cls, splitViewItemWithViewController: content_controller];

        let sidebar_pref = (content_frame.size.width * 0.45)
            .clamp(ui_tokens::SIDEBAR_MIN_WIDTH, ui_tokens::SIDEBAR_MAX_WIDTH);
        let responds_pref: bool =
            msg_send![sidebar_item, respondsToSelector: sel!(setPreferredThickness:)];
        if responds_pref {
            let _: () = msg_send![sidebar_item, setPreferredThickness: sidebar_pref];
        }
        let responds_min: bool =
            msg_send![sidebar_item, respondsToSelector: sel!(setMinimumThickness:)];
        if responds_min {
            let _: () = msg_send![sidebar_item, setMinimumThickness: ui_tokens::SIDEBAR_MIN_WIDTH];
        }
        let responds_behavior: bool =
            msg_send![sidebar_item, respondsToSelector: sel!(setBehavior:)];
        if responds_behavior {
            let _: () = msg_send![sidebar_item, setBehavior: 1_isize];
        }

        let _: () = msg_send![split_controller, addSplitViewItem: sidebar_item];
        let _: () = msg_send![split_controller, addSplitViewItem: content_item];

        let split_view: Id = msg_send![split_controller, view];
        let _: () = msg_send![split_view, setFrame: content_frame];
        let _: () = msg_send![
            split_view,
            setAutoresizingMask: NSVIEW_WIDTH_SIZABLE | NSVIEW_HEIGHT_SIZABLE
        ];
        let responds_vertical: bool = msg_send![split_view, respondsToSelector: sel!(setVertical:)];
        if responds_vertical {
            let _: () = msg_send![split_view, setVertical: true];
        }
        // Divider polish: thin (1pt) + subtle separator color.
        // Guard with respondsToSelector because [splitController view]
        // may return a plain NSView wrapper rather than NSSplitView.
        let responds_divider: bool =
            msg_send![split_view, respondsToSelector: sel!(setDividerStyle:)];
        if responds_divider {
            let _: () = msg_send![split_view, setDividerStyle: 1_isize]; // NSSplitViewDividerStyleThin
            let ns_color = Class::get("NSColor").unwrap();
            let divider_color: Id = msg_send![ns_color, separatorColor];
            let responds_divider_color: bool =
                msg_send![split_view, respondsToSelector: sel!(setDividerColor:)];
            if responds_divider_color && !divider_color.is_null() {
                let _: () = msg_send![split_view, setDividerColor: divider_color];
            }
        }
        add_subview(glass_content_view, split_view);
        // Ensure header controls stay above the split view content.
        add_subview(glass_content_view, header_bg);
        add_subview(header_bg, header_separator);
        add_subview(header_bg, header_controls);

        let inner_pad = ui_tokens::EDGE_PADDING_TIGHT;
        let drawer_frame = CGRect::new(
            &CGPoint::new(inner_pad, inner_pad),
            &CGSize::new(
                (content_frame.size.width - inner_pad * 2.0).max(0.0),
                (content_frame.size.height - inner_pad * 2.0).max(0.0),
            ),
        );

        // Drawer scroll view + stack
        let drawer_scroll: Id = msg_send![ns_scroll, alloc];
        let drawer_scroll: Id = msg_send![drawer_scroll, initWithFrame: drawer_frame];
        // Keep scrolling enabled; hide scrollbars via overlay + autohide.
        let _: () = msg_send![drawer_scroll, setHasVerticalScroller: true];
        let _: () = msg_send![drawer_scroll, setDrawsBackground: false];
        let _: () = msg_send![drawer_scroll, setAutohidesScrollers: true];
        // NSScrollerStyleOverlay == 1
        let _: () = msg_send![drawer_scroll, setScrollerStyle: 1_isize];
        let _: () = msg_send![
            drawer_scroll,
            setAutoresizingMask: NSVIEW_WIDTH_SIZABLE | NSVIEW_HEIGHT_SIZABLE
        ];

        let drawer_container = create_vertical_stack_view(CGRect::new(
            &CGPoint::new(0.0, 0.0),
            &CGSize::new(drawer_frame.size.width, drawer_frame.size.height),
        ));
        // Document views inside NSScrollView must NOT be height-resizable, otherwise AppKit will
        // keep them pinned to the clip view height and effectively disable scrolling.
        let _: () = msg_send![drawer_container, setAutoresizingMask: NSVIEW_WIDTH_SIZABLE];
        let _: () = msg_send![drawer_scroll, setDocumentView: drawer_container];
        add_subview(sidebar_view, drawer_scroll);

        let drawer_edge_frame = CGRect::new(
            &CGPoint::new(
                drawer_frame.origin.x,
                drawer_frame.origin.y + drawer_frame.size.height - 18.0,
            ),
            &CGSize::new(drawer_frame.size.width, 18.0),
        );
        let drawer_edge_effect = create_scroll_edge_effect(drawer_edge_frame);
        let _: () = msg_send![
            drawer_edge_effect,
            setAutoresizingMask: NSVIEW_WIDTH_SIZABLE | NSVIEW_MIN_Y_MARGIN
        ];
        add_subview(sidebar_view, drawer_edge_effect);

        // Agent scroll view + stack
        let agent_scroll_frame_bottom = agent_input_height + inner_pad + content_gap;
        let agent_scroll_frame_top = (content_frame.size.height - inner_pad).max(0.0);
        let agent_scroll_frame = CGRect::new(
            &CGPoint::new(inner_pad, agent_scroll_frame_bottom),
            &CGSize::new(
                (content_frame.size.width - inner_pad * 2.0).max(0.0),
                (agent_scroll_frame_top - agent_scroll_frame_bottom).max(0.0),
            ),
        );
        let agent_scroll: Id = msg_send![ns_scroll, alloc];
        let agent_scroll: Id = msg_send![agent_scroll, initWithFrame: agent_scroll_frame];
        // Keep scrolling enabled; hide scrollbars via overlay + autohide.
        let _: () = msg_send![agent_scroll, setHasVerticalScroller: true];
        let _: () = msg_send![agent_scroll, setDrawsBackground: false];
        let _: () = msg_send![agent_scroll, setAutohidesScrollers: true];
        // NSScrollerStyleOverlay == 1
        let _: () = msg_send![agent_scroll, setScrollerStyle: 1_isize];
        let _: () = msg_send![
            agent_scroll,
            setAutoresizingMask: NSVIEW_WIDTH_SIZABLE | NSVIEW_HEIGHT_SIZABLE
        ];
        let agent_container = create_flipped_vertical_stack_view(CGRect::new(
            &CGPoint::new(0.0, 0.0),
            &CGSize::new(
                agent_scroll_frame.size.width,
                agent_scroll_frame.size.height,
            ),
        ));
        // Same rule: keep the document view width-resizable, but let its height expand to content.
        let _: () = msg_send![agent_container, setAutoresizingMask: NSVIEW_WIDTH_SIZABLE];
        let _: () = msg_send![agent_scroll, setDocumentView: agent_container];
        add_subview(content_view, agent_scroll);

        // Drawer footer (search)
        let search_x = content_frame.origin.x;
        let search_w = content_frame.size.width.max(160.0);
        let footer_base_y = content_bounds.origin.y;

        // Search label removed: NSSearchField.setPlaceholderString("Filter transcripts")
        // on the field below already renders the same text; the redundant label
        // produced a visual duplicate/ghosting under Liquid Glass (Image #16/#17).

        let ns_search = Class::get("NSSearchField").unwrap();
        let search_field: Id = msg_send![ns_search, alloc];
        let search_frame = CGRect::new(
            &CGPoint::new(search_x, footer_base_y + 12.0),
            &CGSize::new(search_w, 24.0),
        );
        let search_field: Id = msg_send![search_field, initWithFrame: search_frame];
        let placeholder = ns_string("Filter transcripts");
        let _: () = msg_send![search_field, setPlaceholderString: placeholder];
        let _: () = msg_send![search_field, setDelegate: action_handler];
        let _: () = msg_send![search_field, setTarget: action_handler];
        let _: () = msg_send![search_field, setAction: sel!(onSearchChanged:)];
        let search_cell: Id = msg_send![search_field, cell];
        if !search_cell.is_null() {
            let supports_immediate: bool =
                msg_send![search_cell, respondsToSelector: sel!(setSendsSearchStringImmediately:)];
            if supports_immediate {
                let _: () = msg_send![search_cell, setSendsSearchStringImmediately: true];
            }
            let supports_whole: bool =
                msg_send![search_cell, respondsToSelector: sel!(setSendsWholeSearchString:)];
            if supports_whole {
                let _: () = msg_send![search_cell, setSendsWholeSearchString: false];
            }
        }
        let _: () = msg_send![
            search_field,
            setAutoresizingMask: NSVIEW_WIDTH_SIZABLE | NSVIEW_MAX_Y_MARGIN
        ];
        set_focus_ring(search_field);
        add_subview(glass_content_view, search_field);

        let footer_brand_w = ui_tokens::CHAT_TITLE_LABEL_WIDTH;
        let footer_brand_h = 16.0;
        let footer_brand_frame = CGRect::new(
            &CGPoint::new(
                content_bounds.origin.x + content_bounds.size.width - content_pad - footer_brand_w,
                content_bounds.origin.y + ((footer_height - footer_brand_h) / 2.0).max(4.0),
            ),
            &CGSize::new(footer_brand_w, footer_brand_h),
        );
        let title_label = create_label(LabelConfig {
            frame: footer_brand_frame,
            text: "CodeScribe".to_string(),
            font_size: ui_tokens::SMALL_FONT_SIZE,
            bold: true,
            text_color: color_secondary_label(),
            background_color: None,
            selectable: false,
            editable: false,
        });
        let _: () = msg_send![
            title_label,
            setAutoresizingMask: NSVIEW_MIN_X_MARGIN | NSVIEW_MAX_Y_MARGIN
        ];
        add_subview(glass_content_view, title_label);

        // Agent input bar
        let drop_target_cls = drop_target_view_class();
        let input_bar: Id = msg_send![drop_target_cls, alloc];
        // Sit flush with the footer edge to match native rounded search/input controls.
        let input_bar_y = (ui_tokens::FOOTER_INSET - 4.0).max(0.0);
        let input_frame = CGRect::new(
            &CGPoint::new(inner_pad, input_bar_y),
            &CGSize::new(
                (content_frame.size.width - inner_pad * 2.0).max(0.0),
                agent_input_height,
            ),
        );
        let input_bar: Id = msg_send![input_bar, initWithFrame: input_frame];
        let _: () = msg_send![input_bar, setWantsLayer: true];
        let _: () =
            msg_send![input_bar, setAutoresizingMask: NSVIEW_WIDTH_SIZABLE | NSVIEW_MAX_Y_MARGIN];
        let drag_types: Id = msg_send![ns_mut_array, array];
        let _: () = msg_send![drag_types, addObject: ns_string("public.file-url")];
        let _: () = msg_send![drag_types, addObject: ns_string("NSFilenamesPboardType")];
        let _: () = msg_send![input_bar, registerForDraggedTypes: drag_types];
        let input_layer: Id = msg_send![input_bar, layer];
        if !input_layer.is_null() {
            let color = ui_colors::input_bar_bg();
            let cg_color: Id = msg_send![color, CGColor];
            let _: () = msg_send![input_layer, setBackgroundColor: cg_color];
            apply_tafla_surface(input_layer, false);
            let border = ui_colors::input_bar_border();
            let cg_border: Id = msg_send![border, CGColor];
            let _: () = msg_send![input_layer, setBorderColor: cg_border];
            let _: () = msg_send![input_layer, setBorderWidth: ui_tokens::SURFACE_BORDER_WIDTH];
            let _: () = msg_send![input_layer, setMasksToBounds: true];
            // Keep the field crisp like native NSSearchField: border-only, no heavy drop shadow.
            let _: () = msg_send![input_layer, setShadowOpacity: 0.0f64];
            let _: () = msg_send![input_layer, setShadowRadius: 0.0f64];
            let _: () = msg_send![input_layer, setShadowOffset: CGSize::new(0.0, 0.0)];
        }
        add_subview(content_view, input_bar);

        let input_width = input_frame.size.width;
        let input_row = chat_input_row_layout(input_width, agent_input_height);
        let text_area_frame = CGRect::new(
            &CGPoint::new(input_row.text_x, input_row.text_y),
            &CGSize::new(input_row.text_width, input_row.text_height),
        );
        let (agent_input_scroll, agent_input_text_view) =
            create_scrollable_text_view(text_area_frame, true);
        let _: () = msg_send![
            agent_input_scroll,
            setAutoresizingMask: NSVIEW_WIDTH_SIZABLE | NSVIEW_HEIGHT_SIZABLE
        ];
        let ns_font = Class::get("NSFont").unwrap();
        let jb_name = ns_string("JetBrainsMono-Regular");
        let jb_font: Id = msg_send![ns_font, fontWithName: jb_name size: 13.0f64];
        let text_font: Id = if jb_font.is_null() {
            msg_send![ns_font, monospacedSystemFontOfSize: 13.0f64 weight: 0.0f64]
        } else {
            jb_font
        };
        let _: () = msg_send![agent_input_text_view, setFont: text_font];
        let _: () = msg_send![
            agent_input_text_view,
            setTextContainerInset: CGSize::new(0.0, 4.0)
        ];
        // Plain text: avoid rich text / style surprises when pasting.
        let _: () = msg_send![agent_input_text_view, setRichText: false];
        let _: () = msg_send![agent_input_text_view, setDelegate: action_handler];
        set_focus_ring(agent_input_text_view);
        let _: () = msg_send![input_bar, addSubview: agent_input_scroll];

        // Attach button (file context for Agent) — anchored left.
        let agent_attach_button = create_button(
            CGRect::new(
                &CGPoint::new(input_row.attach_x, input_row.attach_y),
                &CGSize::new(input_row.button_width, input_row.button_height),
            ),
            "",
            button_style::INLINE,
        );
        let has_symbol = set_button_symbol(agent_attach_button, "paperclip");
        if !has_symbol {
            let _: () = msg_send![agent_attach_button, setTitle: ns_string("Attach")];
        }
        style_toolbar_icon_button(agent_attach_button);
        button_set_action(agent_attach_button, action_handler, sel!(onAttachMenu:));
        let _: () = msg_send![
            agent_attach_button,
            setAutoresizingMask: NSVIEW_MAX_X_MARGIN | NSVIEW_MAX_Y_MARGIN
        ];
        set_tooltip(agent_attach_button, "Attach files (assistant context)");
        let _: () = msg_send![input_bar, addSubview: agent_attach_button];

        let agent_send_button = create_button(
            CGRect::new(
                &CGPoint::new(input_row.send_x, input_row.send_y),
                &CGSize::new(input_row.button_width, input_row.button_height),
            ),
            "",
            button_style::INLINE,
        );
        let has_symbol = set_button_symbol(agent_send_button, "arrow.up.circle.fill");
        if !has_symbol {
            let _: () = msg_send![agent_send_button, setTitle: ns_string("Send")];
        }
        style_toolbar_icon_button(agent_send_button);
        button_set_action(agent_send_button, action_handler, sel!(onSend:));
        set_tooltip(agent_send_button, "Send (Enter)");
        let _: () = msg_send![
            agent_send_button,
            setAutoresizingMask: NSVIEW_MIN_X_MARGIN | NSVIEW_MAX_Y_MARGIN
        ];
        let _: () = msg_send![input_bar, addSubview: agent_send_button];

        // Attachment chip strip (horizontal, above input bar, hidden when empty).
        let chip_strip_height = 36.0f64;
        let chip_strip_y = ui_tokens::FOOTER_INSET + agent_input_height + ui_tokens::CONTENT_GAP;
        let chip_strip_frame = CGRect::new(
            &CGPoint::new(inner_pad, chip_strip_y),
            &CGSize::new(
                (content_frame.size.width - inner_pad * 2.0).max(0.0),
                chip_strip_height,
            ),
        );
        let ns_scroll_cls = Class::get("NSScrollView").unwrap();
        let chip_scroll: Id = msg_send![ns_scroll_cls, alloc];
        let chip_scroll: Id = msg_send![chip_scroll, initWithFrame: chip_strip_frame];
        let _: () = msg_send![chip_scroll, setHasVerticalScroller: false];
        let _: () = msg_send![chip_scroll, setHasHorizontalScroller: false];
        let _: () = msg_send![chip_scroll, setDrawsBackground: false];
        let _: () = msg_send![
            chip_scroll,
            setAutoresizingMask: NSVIEW_WIDTH_SIZABLE | NSVIEW_MAX_Y_MARGIN
        ];
        // Horizontal stack view as document view
        let ns_stack_cls = Class::get("NSStackView").unwrap();
        let chip_stack: Id = msg_send![ns_stack_cls, alloc];
        let stack_inner = CGRect::new(
            &CGPoint::new(0.0, 0.0),
            &CGSize::new(chip_strip_frame.size.width, chip_strip_height),
        );
        let chip_stack: Id = msg_send![chip_stack, initWithFrame: stack_inner];
        // NSUserInterfaceLayoutOrientationHorizontal = 0
        let _: () = msg_send![chip_stack, setOrientation: 0i64];
        let _: () = msg_send![chip_stack, setSpacing: 6.0f64];
        // NSLayoutAttributeLeading alignment = gravity top
        let _: () = msg_send![chip_stack, setAlignment: 7i64]; // NSLayoutAttributeTop
        let _: () = msg_send![chip_scroll, setDocumentView: chip_stack];
        set_hidden(chip_scroll, true);
        add_subview(content_view, chip_scroll);

        // Initial visibility
        set_hidden(agent_scroll, true);
        set_hidden(input_bar, true);
        set_hidden(title_label, true);

        // Phase 3 — store widget pointers into state (short lock scope).
        let (has_messages, desired_tab, status_base_text) = {
            let mut state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
            state.window = Some(window as usize);
            state.window_delegate = Some(window_delegate as usize);
            state.blur_view = Some(blur_view as usize);
            state.split_view_controller = Some(split_controller as usize);
            state.split_sidebar_item = Some(sidebar_item as usize);
            state.split_content_item = Some(content_item as usize);
            state.split_sidebar_container = Some(sidebar_view as usize);
            state.split_content_container = Some(content_view as usize);
            state.title_label = Some(title_label as usize);
            state.status_pill = Some(status_pill as usize);
            state.status_pill_label = Some(status_label as usize);
            state.status_pill_dot = Some(dot as usize);
            state.tab_drawer_button = Some(tab_drawer_button as usize);
            state.tab_agent_button = Some(tab_agent_button as usize);
            state.tab_settings_button = Some(tab_settings_button as usize);
            state.favorites_button = Some(favorites_button as usize);
            state.help_button = Some(help_button as usize);
            state.close_button = Some(close_button as usize);
            state.drawer_scroll_view = Some(drawer_scroll as usize);
            state.drawer_container = Some(drawer_container as usize);
            state.drawer_edge_effect = Some(drawer_edge_effect as usize);
            state.search_field = Some(search_field as usize);
            state.agent_scroll_view = Some(agent_scroll as usize);
            state.agent_container = Some(agent_container as usize);
            state.agent_input_bar = Some(input_bar as usize);
            state.agent_input_scroll_view = Some(agent_input_scroll as usize);
            state.agent_input_text_view = Some(agent_input_text_view as usize);
            state.agent_input_field = None;
            state.agent_attach_button = Some(agent_attach_button as usize);
            state.agent_send_button = Some(agent_send_button as usize);
            state.attachment_chip_strip = Some(chip_scroll as usize);
            state.action_handler = Some(action_handler as usize);
            // Restore persisted zoom level from settings.json.
            if let Some(zoom) = codescribe_core::config::UserSettings::load().chat_zoom {
                state.zoom_level = zoom.clamp(0.75, 2.0);
            }
            let pending_tab = state.pending_tab.take();
            let has_messages = !state.messages.is_empty();
            let desired_tab = if let Some(tab) = pending_tab {
                tab
            } else if has_messages {
                Tab::Agent
            } else {
                state.active_tab
            };
            state.active_tab = desired_tab;
            let status_base_text = state.status_base_text.clone();
            (has_messages, desired_tab, status_base_text)
        }; // OVERLAY_STATE released — safe to perform AppKit window operations.

        // Phase 4 — show window (no lock held; avoids nested-runloop deadlock).
        window_set_alpha(window, 0.0);
        present_shared_shell_panel(window);
        crate::ui_helpers::animate_fade(window, 1.0, 0.2);
        let is_visible: bool = msg_send![window, isVisible];
        let alpha: f64 = msg_send![window, alphaValue];
        debug!(
            "Voice chat overlay visible={} alpha={:.2}",
            is_visible, alpha
        );

        // Safety fallback: ensure the overlay becomes visible even if the fade animation stalls.
        let window_ptr = window as usize;
        thread::spawn(move || {
            thread::sleep(Duration::from_millis(250));
            Queue::main().exec_async(move || {
                let expected = {
                    let state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
                    state.window
                };
                if expected != Some(window_ptr) {
                    return;
                }
                let ns_window = Class::get("NSWindow").unwrap();
                let window = window_ptr as Id;
                let is_window: bool = msg_send![window, isKindOfClass: ns_window];
                if !is_window {
                    return;
                }
                let _: () = msg_send![window, setAlphaValue: 1.0f64];
                present_shared_shell_panel(window);
            });
        });

        // Phase 5 — post-show updates.
        api::refresh_drawer();
        api::update_voice_chat_status(&status_base_text);
        update_active_tab_impl(desired_tab);
        if has_messages || matches!(desired_tab, Tab::Agent) {
            let mut state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
            api::update_chat_view_with_state(&mut state, true);
        }
    }
}

fn create_scroll_edge_effect(frame: CGRect) -> Id {
    unsafe {
        let ns_view = Class::get("NSView").unwrap();
        let ns_color = Class::get("NSColor").unwrap();
        let ns_array = Class::get("NSArray").unwrap();
        let gradient_cls = Class::get("CAGradientLayer");

        let view: Id = msg_send![ns_view, alloc];
        let view: Id = msg_send![view, initWithFrame: frame];
        let _: () = msg_send![view, setWantsLayer: true];
        let layer: Id = msg_send![view, layer];
        if layer.is_null() {
            return view;
        }

        if let Some(gradient_cls) = gradient_cls {
            let gradient: Id = msg_send![gradient_cls, layer];
            let base: Id = msg_send![ns_color, separatorColor];
            let top_color: Id = msg_send![base, colorWithAlphaComponent: 0.0f64];
            let edge_alpha = if crate::ui_helpers::glass_effect_supported() {
                0.08f64
            } else {
                0.14f64
            };
            let bottom_color: Id = msg_send![base, colorWithAlphaComponent: edge_alpha];
            let cg_top: Id = msg_send![top_color, CGColor];
            let cg_bottom: Id = msg_send![bottom_color, CGColor];
            let color_objs: [Id; 2] = [cg_top, cg_bottom];
            let colors: Id = msg_send![
                ns_array,
                arrayWithObjects: color_objs.as_ptr()
                count: color_objs.len()
            ];
            let _: () = msg_send![gradient, setColors: colors];
            let _: () = msg_send![gradient, setStartPoint: CGPoint::new(0.5, 1.0)];
            let _: () = msg_send![gradient, setEndPoint: CGPoint::new(0.5, 0.0)];
            let gradient_frame = CGRect::new(
                &CGPoint::new(0.0, 0.0),
                &CGSize::new(frame.size.width, frame.size.height),
            );
            let _: () = msg_send![gradient, setFrame: gradient_frame];
            let _: () = msg_send![layer, addSublayer: gradient];
        }

        view
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shortcuts_lines_reflect_modifiers() {
        let (hold, toggle) = shortcuts_lines(ModeHotkeyBindings {
            dictation: ShortcutBinding::HoldCtrlAlt,
            formatting: ShortcutBinding::Disabled,
            assistive: ShortcutBinding::DoubleRightOption,
        });
        assert!(hold.contains("Ctrl+Option"));
        assert!(toggle.contains("Right"));
    }
}
