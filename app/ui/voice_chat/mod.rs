//! Voice Chat UI overlay for displaying streaming responses.
//!
//! This module provides a floating overlay window with:
//! - Drawer tab: clipboard-style transcription cards
//! - Transcription tab: live transcription preview for raw dictation
//! - Agent tab: chat bubbles with streaming LLM responses

mod api;
mod handlers;
mod state;

// Re-export public API
pub use api::{
    add_voice_chat_error_message, add_voice_chat_user_message, append_transcription_delta,
    append_voice_chat_assistant_delta, append_voice_chat_user_delta, clear_transcription_text,
    clear_voice_chat_text, filter_drawer, hide_voice_chat_overlay, is_auto_send_enabled,
    is_conversation_active, is_voice_chat_overlay_visible, refresh_drawer,
    reset_voice_chat_activity, send_voice_chat_draft, set_transcription_text,
    set_voice_chat_send_callback, set_voice_chat_sending, set_voice_chat_target_app,
    set_voice_chat_text, set_voice_chat_user_text, show_agent_tab, show_drawer_tab,
    show_settings_tab, show_transcription_tab, update_conversation_state, update_drawer_after_save,
    update_voice_chat_context_summary, update_voice_chat_status,
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

use crate::config::{HoldMods, ToggleTrigger};
use crate::os::hotkeys::{get_hold_mods, get_toggle_trigger};
use crate::ui::bootstrap;

use crate::ui_helpers::{
    LabelConfig, NS_FLOATING_WINDOW_LEVEL, add_subview, button_set_action, button_style,
    color_clear, color_label, color_secondary_label, create_button,
    create_flipped_vertical_stack_view, create_glass_effect_view, create_label,
    create_scrollable_text_view, create_vertical_stack_view, layout_region_frame_for_view,
    ns_string, set_button_symbol, set_focus_ring, set_hidden, set_tooltip,
    style_toolbar_icon_button, ui_colors, ui_tokens, window_set_alpha, window_show,
};

use api::update_active_tab_impl;
use handlers::{action_handler_class, overlay_window_class, window_delegate_class};
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

const CACORNER_MIN_X_MIN_Y: u64 = 1 << 0;
const CACORNER_MAX_X_MIN_Y: u64 = 1 << 1;

pub(super) fn shortcuts_lines(hold: HoldMods, toggle: ToggleTrigger) -> (String, String) {
    let hold_line = match hold {
        HoldMods::Ctrl => "Hold Ctrl — record",
        HoldMods::CtrlAlt => "Hold Ctrl+Option — record",
        HoldMods::CtrlShift => "Hold Ctrl+Shift — record",
        HoldMods::CtrlCmd => "Hold Ctrl+Cmd — record",
    };

    let toggle_line = match toggle {
        ToggleTrigger::DoubleOption => "⌥⌥ — toggle • Right ⌥⌥ — AI",
        ToggleTrigger::DoubleLeftOption => "Left ⌥⌥ — toggle",
        ToggleTrigger::DoubleRightOption => "Right ⌥⌥ — AI",
        ToggleTrigger::DoubleCtrl => "Ctrl Ctrl — toggle (raw)",
        ToggleTrigger::None => "Toggle disabled",
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
                // Ensure the overlay shows up even when the user is in a fullscreen Space.
                let collection_behavior = NSWindowCollectionBehavior::CanJoinAllSpaces
                    | NSWindowCollectionBehavior::FullScreenAuxiliary;
                let _: () = msg_send![window, setCollectionBehavior: collection_behavior];

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
        let style_mask = NSWindowStyleMask::Borderless
            | NSWindowStyleMask::FullSizeContentView
            | NSWindowStyleMask::Resizable;
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
        let _: () = msg_send![window, setContentMinSize: CGSize::new(380.0, 360.0)];
        // Prevent "infinite" resizing; cap at the current screen's visible frame.
        let ns_screen = Class::get("NSScreen").unwrap();
        let screen: Id = msg_send![ns_screen, mainScreen];
        if !screen.is_null() {
            let visible: CGRect = msg_send![screen, visibleFrame];
            let _: () = msg_send![window, setContentMaxSize: visible.size];
        }
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
        let _: () = msg_send![blur_view, setMaterial: NSVisualEffectMaterial::WindowBackground];
        let _: () = msg_send![blur_view, setBlendingMode: NSVisualEffectBlendingMode::BehindWindow];
        let _: () = msg_send![blur_view, setState: NSVisualEffectState::Active];
        let _: () = msg_send![blur_view, setWantsLayer: true];
        let _: () = msg_send![
            blur_view,
            setAutoresizingMask: NSVIEW_WIDTH_SIZABLE | NSVIEW_HEIGHT_SIZABLE
        ];
        let layer: Id = msg_send![blur_view, layer];
        if !layer.is_null() {
            let _: () = msg_send![layer, setCornerRadius: ui_tokens::CORNER_RADIUS_LG];
            let _: () = msg_send![layer, setMasksToBounds: true];
        }
        add_subview(content_view, blur_view);
        let bounds: CGRect = msg_send![blur_view, bounds];
        let content_bounds = layout_region_frame_for_view(blur_view).unwrap_or(bounds);

        let action_handler_class = action_handler_class();
        let action_handler: Id = msg_send![action_handler_class, new];

        let content_pad = ui_tokens::EDGE_PADDING;

        let header_height = ui_tokens::HEADER_HEIGHT;
        let footer_height = ui_tokens::FOOTER_HEIGHT;
        // Start compact; grows dynamically as the user types/pastes more content.
        // Agent input starts compact and can grow with content (see `resize_agent_input_locked`).
        let agent_input_height = ui_tokens::AGENT_INPUT_HEIGHT;

        // Header background (glass if available, fallback to subtle layer)
        let header_frame = CGRect::new(
            &CGPoint::new(
                content_bounds.origin.x,
                content_bounds.origin.y + content_bounds.size.height - header_height,
            ),
            &CGSize::new(content_bounds.size.width.max(0.0), header_height),
        );
        let header_bg: Id = if let Some(container_cls) = Class::get("NSGlassEffectContainerView") {
            let container: Id = msg_send![container_cls, alloc];
            let container: Id = msg_send![container, initWithFrame: header_frame];
            let _: () = msg_send![container, setWantsLayer: true];

            let glass_frame = CGRect::new(
                &CGPoint::new(0.0, 0.0),
                &CGSize::new(header_frame.size.width, header_height),
            );
            let glass: Id = create_glass_effect_view(glass_frame, NSVisualEffectMaterial::Titlebar);
            let _: () =
                msg_send![glass, setAutoresizingMask: NSVIEW_WIDTH_SIZABLE | NSVIEW_HEIGHT_SIZABLE];
            add_subview(container, glass);
            container
        } else {
            create_glass_effect_view(header_frame, NSVisualEffectMaterial::HeaderView)
        };
        let _: () = msg_send![
            header_bg,
            setAutoresizingMask: NSVIEW_WIDTH_SIZABLE | NSVIEW_MIN_Y_MARGIN
        ];
        let header_layer: Id = msg_send![header_bg, layer];
        if !header_layer.is_null() {
            let radius = ui_tokens::CORNER_RADIUS_LG;
            let _: () = msg_send![header_layer, setCornerRadius: radius];
            let responds_masked: bool =
                msg_send![header_layer, respondsToSelector: sel!(setMaskedCorners:)];
            if responds_masked {
                let corners = CACORNER_MIN_X_MIN_Y | CACORNER_MAX_X_MIN_Y;
                let _: () = msg_send![header_layer, setMaskedCorners: corners];
            }
            let _: () = msg_send![header_layer, setMasksToBounds: true];
        }
        add_subview(blur_view, header_bg);

        let title_x = header_frame.origin.x + ui_tokens::EDGE_PADDING_TIGHT;
        let title_y = header_frame.origin.y + ((header_height - 20.0) / 2.0).max(0.0);
        // Give the tab control more room to avoid truncation ("Dr..." / "A...").
        let title_w = ui_tokens::TITLE_LABEL_WIDTH;
        let title_label = create_label(LabelConfig {
            frame: CGRect::new(&CGPoint::new(title_x, title_y), &CGSize::new(title_w, 20.0)),
            text: "CodeScribe".to_string(),
            font_size: ui_tokens::TITLE_FONT_SIZE,
            bold: true,
            text_color: color_label(),
            background_color: None,
            selectable: false,
            editable: false,
        });
        let _: () = msg_send![
            title_label,
            setAutoresizingMask: NSVIEW_MAX_X_MARGIN | NSVIEW_MIN_Y_MARGIN
        ];
        add_subview(blur_view, title_label);

        // Header right-side controls (right-aligned, consistent spacing).
        let btn_w = ui_tokens::HEADER_BUTTON_SIZE;
        let btn_h = ui_tokens::HEADER_BUTTON_SIZE;
        let gap = ui_tokens::HEADER_BUTTON_GAP;
        let right_pad = ui_tokens::EDGE_PADDING_TIGHT;
        let header_btn_y = header_frame.origin.y + ((header_height - btn_h) / 2.0).max(0.0);

        let mut x = header_frame.origin.x + header_frame.size.width - right_pad - btn_w;
        let close_button_x = x;
        x -= gap + btn_w;
        let more_button_x = x;
        x -= gap + btn_w;
        let favorites_button_x = x;
        x -= gap + btn_w;
        let export_button_x = x;

        // Keep the tab control between the title and the right-side icon cluster.
        let right_cluster_start_x = export_button_x;
        let tab_x = title_x + title_w + 10.0;
        let status_pill_w = ui_tokens::STATUS_PILL_WIDTH;
        let tab_btn_w = (btn_w - 2.0).max(24.0);
        let tab_gap = (gap - 2.0).max(6.0);
        let tab_cluster_w = tab_btn_w * 4.0 + tab_gap * 3.0;
        let status_pill_x =
            (right_cluster_start_x - gap - status_pill_w).max(tab_x + tab_cluster_w + tab_gap);
        let tab_cluster_x = tab_x;

        let tab_drawer_button = create_button(
            CGRect::new(
                &CGPoint::new(tab_cluster_x, header_btn_y),
                &CGSize::new(tab_btn_w, btn_h),
            ),
            "",
            button_style::INLINE,
        );
        let _ = set_button_symbol(tab_drawer_button, "tray.full");
        style_toolbar_icon_button(tab_drawer_button);
        button_set_action(tab_drawer_button, action_handler, sel!(onTabDrawer:));
        set_tooltip(tab_drawer_button, "Drawer");
        let _: () = msg_send![
            tab_drawer_button,
            setAutoresizingMask: NSVIEW_MAX_X_MARGIN | NSVIEW_MIN_Y_MARGIN
        ];
        set_focus_ring(tab_drawer_button);
        add_subview(blur_view, tab_drawer_button);

        let tab_transcription_button = create_button(
            CGRect::new(
                &CGPoint::new(tab_cluster_x + tab_btn_w + tab_gap, header_btn_y),
                &CGSize::new(tab_btn_w, btn_h),
            ),
            "",
            button_style::INLINE,
        );
        let _ = set_button_symbol(tab_transcription_button, "waveform");
        style_toolbar_icon_button(tab_transcription_button);
        button_set_action(
            tab_transcription_button,
            action_handler,
            sel!(onTabTranscription:),
        );
        set_tooltip(tab_transcription_button, "Transcription");
        let _: () = msg_send![
            tab_transcription_button,
            setAutoresizingMask: NSVIEW_MAX_X_MARGIN | NSVIEW_MIN_Y_MARGIN
        ];
        set_focus_ring(tab_transcription_button);
        add_subview(blur_view, tab_transcription_button);

        let tab_agent_button = create_button(
            CGRect::new(
                &CGPoint::new(tab_cluster_x + (tab_btn_w + tab_gap) * 2.0, header_btn_y),
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
        set_focus_ring(tab_agent_button);
        add_subview(blur_view, tab_agent_button);

        let tab_settings_button = create_button(
            CGRect::new(
                &CGPoint::new(tab_cluster_x + (tab_btn_w + tab_gap) * 3.0, header_btn_y),
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
        set_focus_ring(tab_settings_button);
        add_subview(blur_view, tab_settings_button);

        // Status pill (global status: Idle / Listening / Processing / Error).
        let status_pill_h = ui_tokens::STATUS_PILL_HEIGHT;
        let status_pill_y =
            header_frame.origin.y + ((header_height - status_pill_h) / 2.0).max(0.0);
        let status_pill_frame = CGRect::new(
            &CGPoint::new(status_pill_x, status_pill_y),
            &CGSize::new(status_pill_w, status_pill_h),
        );
        let status_pill: Id =
            create_glass_effect_view(status_pill_frame, NSVisualEffectMaterial::HUDWindow);
        let status_layer: Id = msg_send![status_pill, layer];
        if !status_layer.is_null() {
            let _: () = msg_send![
                status_layer,
                setCornerRadius: (status_pill_h / 2.0).max(8.0)
            ];
            let _: () = msg_send![status_layer, setMasksToBounds: true];
        }
        let _: () = msg_send![
            status_pill,
            setAutoresizingMask: NSVIEW_MIN_X_MARGIN | NSVIEW_MIN_Y_MARGIN
        ];

        let dot_size = ui_tokens::STATUS_DOT_SIZE;
        let dot: Id = msg_send![Class::get("NSView").unwrap(), alloc];
        let dot: Id = msg_send![
            dot,
            initWithFrame: CGRect::new(
                &CGPoint::new(8.0, (status_pill_h - dot_size) / 2.0),
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
                &CGPoint::new(18.0, 2.0),
                &CGSize::new(status_pill_w - 22.0, status_pill_h - 4.0),
            ),
            text: "Idle".to_string(),
            font_size: ui_tokens::SMALL_FONT_SIZE,
            bold: true,
            text_color: crate::ui_helpers::color_white(0.9),
            background_color: None,
            selectable: false,
            editable: false,
        });
        let _: () = msg_send![
            status_label,
            setAutoresizingMask: NSVIEW_WIDTH_SIZABLE | NSVIEW_MIN_Y_MARGIN
        ];
        add_subview(status_pill, status_label);
        add_subview(blur_view, status_pill);

        let export_button = create_button(
            CGRect::new(
                &CGPoint::new(export_button_x, header_btn_y),
                &CGSize::new(btn_w, btn_h),
            ),
            "",
            button_style::INLINE,
        );
        let has_symbol = set_button_symbol(export_button, "arrow.down.to.line");
        if !has_symbol {
            let _: () = msg_send![export_button, setTitle: ns_string("Export")];
        }
        style_toolbar_icon_button(export_button);
        button_set_action(export_button, action_handler, sel!(onExportMenu:));
        set_tooltip(export_button, "Export conversation (Markdown)");
        let _: () = msg_send![
            export_button,
            setAutoresizingMask: NSVIEW_MIN_X_MARGIN | NSVIEW_MIN_Y_MARGIN
        ];
        add_subview(blur_view, export_button);

        // Drawer favorites filter (hearts on/off)
        let favorites_button = create_button(
            CGRect::new(
                &CGPoint::new(favorites_button_x, header_btn_y),
                &CGSize::new(btn_w, btn_h),
            ),
            "",
            button_style::INLINE,
        );
        let _ = set_button_symbol(favorites_button, "heart");
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
        add_subview(blur_view, favorites_button);

        let more_button = create_button(
            CGRect::new(
                &CGPoint::new(more_button_x, header_btn_y),
                &CGSize::new(btn_w, btn_h),
            ),
            "",
            button_style::INLINE,
        );
        let has_symbol = set_button_symbol(more_button, "ellipsis");
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
        add_subview(blur_view, more_button);

        let close_button = create_button(
            CGRect::new(
                &CGPoint::new(close_button_x, header_btn_y),
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
        let _: () = msg_send![
            close_button,
            setAutoresizingMask: NSVIEW_MIN_X_MARGIN | NSVIEW_MIN_Y_MARGIN
        ];
        add_subview(blur_view, close_button);

        // Drawer/Agent split view (system sidebar glass)
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
        let sidebar_view: Id = msg_send![ns_visual, alloc];
        let sidebar_view: Id = msg_send![
            sidebar_view,
            initWithFrame: CGRect::new(
                &CGPoint::new(0.0, 0.0),
                &CGSize::new(content_frame.size.width, content_frame.size.height),
            )
        ];
        let _: () = msg_send![sidebar_view, setMaterial: NSVisualEffectMaterial::Sidebar];
        let _: () =
            msg_send![sidebar_view, setBlendingMode: NSVisualEffectBlendingMode::BehindWindow];
        let _: () = msg_send![sidebar_view, setState: NSVisualEffectState::Active];
        let _: () = msg_send![sidebar_view, setWantsLayer: true];
        let sidebar_layer: Id = msg_send![sidebar_view, layer];
        if !sidebar_layer.is_null() {
            let _: () = msg_send![sidebar_layer, setCornerRadius: ui_tokens::CORNER_RADIUS_MD];
            let _: () = msg_send![sidebar_layer, setMasksToBounds: true];
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
        add_subview(blur_view, split_view);

        let settings_view = bootstrap::attach_settings_view(blur_view, content_frame);
        if let Some(settings_view) = settings_view {
            set_hidden(settings_view, true);
        }

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

        // Transcription scroll view (live dictation preview)
        let (transcription_scroll, transcription_text_view) =
            create_scrollable_text_view(drawer_frame, false);
        let _: () = msg_send![
            transcription_scroll,
            setAutoresizingMask: NSVIEW_WIDTH_SIZABLE | NSVIEW_HEIGHT_SIZABLE
        ];
        let ns_font = Class::get("NSFont").unwrap();
        let text_font: Id = msg_send![ns_font, systemFontOfSize: ui_tokens::BODY_FONT_SIZE];
        let _: () = msg_send![transcription_text_view, setFont: text_font];
        let _: () = msg_send![transcription_text_view, setRichText: false];
        let _: () = msg_send![transcription_text_view, setEditable: false];
        let _: () = msg_send![transcription_text_view, setSelectable: true];
        add_subview(sidebar_view, transcription_scroll);

        let transcription_edge_frame = CGRect::new(
            &CGPoint::new(
                drawer_frame.origin.x,
                drawer_frame.origin.y + drawer_frame.size.height - 18.0,
            ),
            &CGSize::new(drawer_frame.size.width, 18.0),
        );
        let transcription_edge_effect = create_scroll_edge_effect(transcription_edge_frame);
        let _: () = msg_send![
            transcription_edge_effect,
            setAutoresizingMask: NSVIEW_WIDTH_SIZABLE | NSVIEW_MIN_Y_MARGIN
        ];
        set_hidden(transcription_edge_effect, true);
        add_subview(sidebar_view, transcription_edge_effect);

        // Transcription empty-state placeholder
        let placeholder_view: Id = msg_send![Class::get("NSView").unwrap(), alloc];
        let placeholder_view: Id = msg_send![placeholder_view, initWithFrame: drawer_frame];
        let _: () = msg_send![placeholder_view, setWantsLayer: true];
        let _: () = msg_send![
            placeholder_view,
            setAutoresizingMask: NSVIEW_WIDTH_SIZABLE | NSVIEW_HEIGHT_SIZABLE
        ];

        let placeholder_label = create_label(LabelConfig {
            frame: CGRect::new(
                &CGPoint::new(0.0, drawer_frame.size.height / 2.0 + 12.0),
                &CGSize::new(drawer_frame.size.width, 20.0),
            ),
            text: "Press hotkey to start".to_string(),
            font_size: ui_tokens::BODY_FONT_SIZE,
            bold: true,
            text_color: color_secondary_label(),
            background_color: None,
            selectable: false,
            editable: false,
        });
        let _: () = msg_send![placeholder_label, setAlignment: 1_isize];
        add_subview(placeholder_view, placeholder_label);

        let ns_progress = Class::get("NSProgressIndicator").unwrap();
        let line_w = ui_tokens::PLACEHOLDER_LINE_WIDTH;
        let line_h = ui_tokens::PLACEHOLDER_LINE_HEIGHT;
        let line_x = (drawer_frame.size.width - line_w) / 2.0;
        let line_y = drawer_frame.size.height / 2.0 - 6.0;
        let line_frame = CGRect::new(&CGPoint::new(line_x, line_y), &CGSize::new(line_w, line_h));
        let line: Id = msg_send![ns_progress, alloc];
        let line: Id = msg_send![line, initWithFrame: line_frame];
        let _: () = msg_send![line, setIndeterminate: true];
        let _: () = msg_send![line, setStyle: 0_isize]; // NSProgressIndicatorStyleBar
        let _: () = msg_send![line, setDisplayedWhenStopped: false];
        let _: () = msg_send![line, startAnimation: std::ptr::null::<Object>()];
        add_subview(placeholder_view, line);

        set_hidden(placeholder_view, true);
        add_subview(sidebar_view, placeholder_view);

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

        // Drawer footer (search + shortcuts helper)
        let help_panel_w = ui_tokens::HELP_PANEL_WIDTH;
        let help_panel_h = footer_height - ui_tokens::FOOTER_INSET;
        let help_panel_x = content_frame.origin.x + content_frame.size.width - help_panel_w;
        let search_x = content_frame.origin.x;
        let search_w = (help_panel_x - gap - search_x).max(160.0);
        let footer_base_y = content_bounds.origin.y;

        let search_label = create_label(LabelConfig {
            frame: CGRect::new(
                &CGPoint::new(search_x, footer_base_y + footer_height - 20.0),
                &CGSize::new(search_w, 16.0),
            ),
            text: "Filter transcripts".to_string(),
            font_size: ui_tokens::SMALL_FONT_SIZE,
            bold: false,
            text_color: color_secondary_label(),
            background_color: None,
            selectable: false,
            editable: false,
        });
        let _: () = msg_send![
            search_label,
            setAutoresizingMask: NSVIEW_WIDTH_SIZABLE | NSVIEW_MAX_Y_MARGIN
        ];
        add_subview(blur_view, search_label);

        let ns_search = Class::get("NSSearchField").unwrap();
        let search_field: Id = msg_send![ns_search, alloc];
        let search_frame = CGRect::new(
            &CGPoint::new(search_x, footer_base_y + 12.0),
            &CGSize::new(search_w, 24.0),
        );
        let search_field: Id = msg_send![search_field, initWithFrame: search_frame];
        let placeholder = ns_string("Filter transcripts");
        let _: () = msg_send![search_field, setPlaceholderString: placeholder];
        let _: () = msg_send![search_field, setTarget: action_handler];
        let _: () = msg_send![search_field, setAction: sel!(onSearchChanged:)];
        let _: () = msg_send![
            search_field,
            setAutoresizingMask: NSVIEW_WIDTH_SIZABLE | NSVIEW_MAX_Y_MARGIN
        ];
        set_focus_ring(search_field);
        add_subview(blur_view, search_field);

        let help_panel: Id = msg_send![Class::get("NSView").unwrap(), alloc];
        let help_frame = CGRect::new(
            &CGPoint::new(help_panel_x, footer_base_y + 6.0),
            &CGSize::new(help_panel_w, help_panel_h),
        );
        let help_panel: Id = msg_send![help_panel, initWithFrame: help_frame];
        let _: () = msg_send![help_panel, setWantsLayer: true];
        let help_layer: Id = msg_send![help_panel, layer];
        if !help_layer.is_null() {
            let bg = ui_colors::panel_bg();
            let cg: Id = msg_send![bg, CGColor];
            let _: () = msg_send![help_layer, setBackgroundColor: cg];
            let _: () = msg_send![help_layer, setCornerRadius: ui_tokens::CORNER_RADIUS_MD];
        }
        let _: () = msg_send![
            help_panel,
            setAutoresizingMask: NSVIEW_MIN_X_MARGIN | NSVIEW_MAX_Y_MARGIN
        ];
        add_subview(blur_view, help_panel);
        // Default hidden; only visible on Agent tab.
        set_hidden(help_panel, true);

        let (hold_line, toggle_line) = shortcuts_lines(get_hold_mods(), get_toggle_trigger());

        let help_title = create_label(LabelConfig {
            frame: CGRect::new(
                &CGPoint::new(10.0, help_panel_h - 18.0),
                &CGSize::new(help_panel_w - 20.0, 14.0),
            ),
            text: "Shortcuts".to_string(),
            font_size: ui_tokens::SMALL_FONT_SIZE,
            bold: true,
            text_color: color_label(),
            background_color: None,
            selectable: false,
            editable: false,
        });
        add_subview(help_panel, help_title);

        let hold_label = create_label(LabelConfig {
            frame: CGRect::new(
                &CGPoint::new(10.0, help_panel_h - 34.0),
                &CGSize::new(help_panel_w - 20.0, 12.0),
            ),
            text: hold_line.clone(),
            font_size: ui_tokens::MICRO_FONT_SIZE,
            bold: false,
            text_color: color_secondary_label(),
            background_color: None,
            selectable: false,
            editable: false,
        });
        add_subview(help_panel, hold_label);

        let toggle_label = create_label(LabelConfig {
            frame: CGRect::new(
                &CGPoint::new(10.0, help_panel_h - 48.0),
                &CGSize::new(help_panel_w - 20.0, 12.0),
            ),
            text: toggle_line.clone(),
            font_size: ui_tokens::MICRO_FONT_SIZE,
            bold: false,
            text_color: color_secondary_label(),
            background_color: None,
            selectable: false,
            editable: false,
        });
        add_subview(help_panel, toggle_label);
        let shortcuts_tip = format!("{hold_line}\n{toggle_line}");
        set_tooltip(help_panel, &shortcuts_tip);

        // Agent input bar
        let input_bar: Id = msg_send![Class::get("NSView").unwrap(), alloc];
        let input_frame = CGRect::new(
            &CGPoint::new(inner_pad, ui_tokens::FOOTER_INSET),
            &CGSize::new(
                (content_frame.size.width - inner_pad * 2.0).max(0.0),
                agent_input_height,
            ),
        );
        let input_bar: Id = msg_send![input_bar, initWithFrame: input_frame];
        let _: () = msg_send![input_bar, setWantsLayer: true];
        let _: () =
            msg_send![input_bar, setAutoresizingMask: NSVIEW_WIDTH_SIZABLE | NSVIEW_MAX_Y_MARGIN];
        let input_layer: Id = msg_send![input_bar, layer];
        if !input_layer.is_null() {
            let color = ui_colors::input_bar_bg();
            let cg_color: Id = msg_send![color, CGColor];
            let _: () = msg_send![input_layer, setBackgroundColor: cg_color];
            let _: () = msg_send![input_layer, setCornerRadius: ui_tokens::CORNER_RADIUS_LG];
            let border = ui_colors::input_bar_border();
            let cg_border: Id = msg_send![border, CGColor];
            let _: () = msg_send![input_layer, setBorderColor: cg_border];
            let _: () = msg_send![input_layer, setBorderWidth: 1.0f64];
        }
        add_subview(content_view, input_bar);

        let input_width = input_frame.size.width;
        let text_area_frame = CGRect::new(
            &CGPoint::new(12.0, 10.0),
            // Leave room for Attach + Send buttons on the right.
            &CGSize::new((input_width - 140.0).max(120.0), agent_input_height - 20.0),
        );
        let (agent_input_scroll, agent_input_text_view) =
            create_scrollable_text_view(text_area_frame, true);
        let _: () = msg_send![
            agent_input_scroll,
            setAutoresizingMask: NSVIEW_WIDTH_SIZABLE | NSVIEW_HEIGHT_SIZABLE
        ];
        let ns_font = Class::get("NSFont").unwrap();
        let text_font: Id = msg_send![ns_font, systemFontOfSize: 13.0f64];
        let _: () = msg_send![agent_input_text_view, setFont: text_font];
        // Plain text: avoid rich text / style surprises when pasting.
        let _: () = msg_send![agent_input_text_view, setRichText: false];
        let _: () = msg_send![agent_input_text_view, setDelegate: action_handler];
        set_focus_ring(agent_input_text_view);
        let _: () = msg_send![input_bar, addSubview: agent_input_scroll];

        // Attach button (file context for Agent).
        let send_y = ((agent_input_height - 32.0) / 2.0).max(8.0);
        let agent_attach_button = create_button(
            CGRect::new(
                &CGPoint::new((input_width - 120.0).max(0.0), send_y),
                &CGSize::new(36.0, 32.0),
            ),
            "",
            button_style::ROUNDED,
        );
        let has_symbol = set_button_symbol(agent_attach_button, "paperclip");
        if !has_symbol {
            let _: () = msg_send![agent_attach_button, setTitle: ns_string("Attach")];
        }
        button_set_action(agent_attach_button, action_handler, sel!(onAttachMenu:));
        let _: () = msg_send![
            agent_attach_button,
            setAutoresizingMask: NSVIEW_MIN_X_MARGIN | NSVIEW_MAX_Y_MARGIN
        ];
        set_tooltip(agent_attach_button, "Attach files (assistant context)");
        let _: () = msg_send![input_bar, addSubview: agent_attach_button];

        let agent_send_button = create_button(
            CGRect::new(
                &CGPoint::new((input_width - 76.0).max(0.0), send_y),
                &CGSize::new(36.0, 32.0),
            ),
            ">",
            button_style::ROUNDED,
        );
        button_set_action(agent_send_button, action_handler, sel!(onSend:));
        set_tooltip(agent_send_button, "Send (Enter)");
        let _: () = msg_send![
            agent_send_button,
            setAutoresizingMask: NSVIEW_MIN_X_MARGIN | NSVIEW_MAX_Y_MARGIN
        ];
        let _: () = msg_send![input_bar, addSubview: agent_send_button];

        // Initial visibility
        set_hidden(agent_scroll, true);
        set_hidden(input_bar, true);
        set_hidden(transcription_scroll, true);

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
        state.tab_transcription_button = Some(tab_transcription_button as usize);
        state.tab_agent_button = Some(tab_agent_button as usize);
        state.tab_settings_button = Some(tab_settings_button as usize);
        state.favorites_button = Some(favorites_button as usize);
        state.close_button = Some(close_button as usize);
        state.settings_view = settings_view.map(|view| view as usize);
        state.drawer_scroll_view = Some(drawer_scroll as usize);
        state.drawer_container = Some(drawer_container as usize);
        state.drawer_edge_effect = Some(drawer_edge_effect as usize);
        state.search_field = Some(search_field as usize);
        state.search_label = Some(search_label as usize);
        state.help_panel = Some(help_panel as usize);
        state.help_hold_label = Some(hold_label as usize);
        state.help_toggle_label = Some(toggle_label as usize);
        state.agent_scroll_view = Some(agent_scroll as usize);
        state.agent_container = Some(agent_container as usize);
        state.agent_input_bar = Some(input_bar as usize);
        state.agent_input_scroll_view = Some(agent_input_scroll as usize);
        state.agent_input_text_view = Some(agent_input_text_view as usize);
        state.agent_input_field = None;
        state.agent_attach_button = Some(agent_attach_button as usize);
        state.agent_send_button = Some(agent_send_button as usize);
        state.transcription_scroll_view = Some(transcription_scroll as usize);
        state.transcription_text_view = Some(transcription_text_view as usize);
        state.transcription_placeholder = Some(placeholder_view as usize);
        state.transcription_edge_effect = Some(transcription_edge_effect as usize);
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
        let status_text = state.status_text.clone();
        drop(state);
        api::refresh_drawer();
        api::update_voice_chat_status(&status_text);
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
            let bottom_color: Id = msg_send![base, colorWithAlphaComponent: 0.35f64];
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
        let (hold, toggle) = shortcuts_lines(HoldMods::CtrlAlt, ToggleTrigger::DoubleRightOption);
        assert!(hold.contains("Ctrl+Option"));
        assert!(toggle.contains("Right"));
    }
}
