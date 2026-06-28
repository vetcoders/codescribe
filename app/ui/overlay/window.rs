//! NSWindow construction and static UI build for the transcription overlay:
//! Tahoe Liquid Glass blur, header/status/hint labels, spinner, scrollable
//! text view, hover tracking area, and the decision-action button row.

use std::sync::atomic::Ordering;
use std::time::Instant;

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

use super::actions::{OverlayActionButtonRole, action_handler_class, overlay_button_selector};
use super::layout::{
    NS_FLOATING_WINDOW_LEVEL, NSVIEW_HEIGHT_SIZABLE, NSVIEW_MAX_X_MARGIN, NSVIEW_MAX_Y_MARGIN,
    NSVIEW_MIN_Y_MARGIN, NSVIEW_WIDTH_SIZABLE, OVERLAY_BUTTON_HEIGHT, OVERLAY_CONTENT_GAP,
    OVERLAY_HEADER_GAP, OVERLAY_HEADER_HEIGHT, OVERLAY_INFO_HEIGHT, OVERLAY_PADDING,
    OVERLAY_STATUS_HEIGHT, OVERLAY_STATUS_WIDTH, OVERLAY_WINDOW_MAX_HEIGHT_RATIO,
    OVERLAY_WINDOW_MAX_WIDTH, OVERLAY_WINDOW_MIN_HEIGHT, OVERLAY_WINDOW_MIN_WIDTH,
    OVERLAY_WINDOW_WIDTH, compute_overlay_layout_metrics, overlay_bottom_reserved_height,
    resize_overlay_unlocked,
};
use super::state::{
    AUTO_HIDE_GENERATION, AUTO_HIDE_PENDING, FormatPhase, OVERLAY_STATE, OverlaySnapshot,
    TranscriptionActionContractMode,
};
use super::widgets::{
    augment_action_tooltip, copy_action_tooltip, decision_hint_text,
    refresh_action_contract_ui_unlocked, reset_overlay_to_idle_unlocked,
};
use crate::ui_helpers::{
    Id, add_subview, animate_fade, button_set_action, button_style, clamp_overlay_position,
    create_button, create_glass_effect_view_with, create_label, create_scrollable_text_view,
    ns_string, set_glass_effect_content_view, set_hidden, set_text_view_string, set_tooltip,
    ui_colors, ui_tokens, window_discard, window_set_alpha, window_show,
};

const NS_PROGRESS_INDICATOR_STYLE_SPINNING: i64 = 1;
const NSTRACKING_MOUSE_ENTERED_AND_EXITED: u64 = 1 << 0;
const NSTRACKING_ACTIVE_ALWAYS: u64 = 1 << 7;
const NSTRACKING_IN_VISIBLE_RECT: u64 = 1 << 9;

const OVERLAY_HEADER_LABEL: &str = "Codescribe - Dictation Overlay";

static OVERLAY_WINDOW_INIT: std::sync::Once = std::sync::Once::new();
static mut OVERLAY_WINDOW_CLASS: *const Class = std::ptr::null();

extern "C" fn overlay_can_become_key(_this: &Object, _cmd: objc::runtime::Sel) -> bool {
    true
}

/// NSWindow subclass for the dictation overlay.
///
/// The overlay is a borderless floating window. On macOS a borderless
/// `NSWindow` returns `canBecomeKeyWindow = NO` by default, which means its
/// `NSTextView` can never become first responder — so `setEditable: true` is a
/// visual lie: a caret never appears and keystrokes are dropped. Overriding
/// `canBecomeKeyWindow`/`canBecomeMainWindow` lets the user click into the
/// transcript and actually edit it (mirrors `VoiceChatOverlayWindow`).
pub(super) fn overlay_window_class() -> *const Class {
    unsafe {
        OVERLAY_WINDOW_INIT.call_once(|| {
            let superclass = Class::get("NSWindow").expect("NSWindow class missing");
            let mut decl =
                objc::declare::ClassDecl::new("CodescribeDictationOverlayWindow", superclass)
                    .expect("Failed to declare dictation overlay window class");
            decl.add_method(
                sel!(canBecomeKeyWindow),
                overlay_can_become_key as extern "C" fn(&Object, objc::runtime::Sel) -> bool,
            );
            decl.add_method(
                sel!(canBecomeMainWindow),
                overlay_can_become_key as extern "C" fn(&Object, objc::runtime::Sel) -> bool,
            );
            OVERLAY_WINDOW_CLASS = decl.register();
        });
        OVERLAY_WINDOW_CLASS
    }
}

/// Show the transcription overlay window
pub fn show_transcription_overlay() {
    // Cancel any pending auto-hide
    AUTO_HIDE_GENERATION.fetch_add(1, Ordering::SeqCst);
    AUTO_HIDE_PENDING.store(false, Ordering::SeqCst);

    Queue::main().exec_async(|| {
        show_transcription_overlay_impl();
    });
}

fn show_transcription_overlay_impl() {
    unsafe {
        let mut state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());

        // Reuse existing window if any
        if let Some(window_ptr) = state.window {
            // DEADLOCK PREVENTION: extract snapshot, drop lock before AppKit calls.
            let snap = OverlaySnapshot::from_state(&state);
            drop(state);

            let window = window_ptr as Id;
            let _: () = msg_send![window, setLevel: NS_FLOATING_WINDOW_LEVEL];
            window_show(window);
            let new_h = resize_overlay_unlocked(&snap);
            {
                let mut state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
                state.last_applied_height = new_h;
                state.last_layout_resize_at = Instant::now();
                state.pending_layout_resize = false;
            }
            info!("Transcription overlay reused");
            return;
        }

        state.accumulated_text.clear();
        state.raw_text.clear();
        state.last_pass_text.clear();
        state.user_edited = false;
        state.action_contract_mode = TranscriptionActionContractMode::Raw;
        state.format_phase = FormatPhase::Idle;
        drop(state); // Release lock BEFORE heavy AppKit widget creation.

        // Get classes
        let ns_screen_class = Class::get("NSScreen");
        let ns_color_class = Class::get("NSColor");
        let ns_progress_class = Class::get("NSProgressIndicator");
        let ns_tracking_area_class = Class::get("NSTrackingArea");

        // Defensive checks for Cocoa classes
        if ns_screen_class.is_none()
            || ns_color_class.is_none()
            || ns_progress_class.is_none()
            || ns_tracking_area_class.is_none()
        {
            warn!("Failed to get required Cocoa classes");
            return;
        }

        // Keyable borderless subclass so the transcript NSTextView accepts edits.
        let ns_window = overlay_window_class();
        let ns_screen = ns_screen_class.unwrap();
        let ns_color = ns_color_class.unwrap();
        let ns_progress = ns_progress_class.unwrap();
        let ns_tracking_area = ns_tracking_area_class.unwrap();

        // Get screen size to position the overlay
        let main_screen: Id = msg_send![ns_screen, mainScreen];
        if main_screen.is_null() {
            warn!("No main screen available");
            return;
        }
        let visible_frame: CGRect = msg_send![main_screen, visibleFrame];

        // Load config for position logic
        let config = Config::load();

        // Modern compact dimensions for Tahoe-style overlay
        let window_width = OVERLAY_WINDOW_WIDTH;
        let window_height = OVERLAY_WINDOW_MIN_HEIGHT;
        let margin = 20.0;
        let corner_radius = ui_tokens::SURFACE_RADIUS;
        let max_height =
            (visible_frame.size.height * OVERLAY_WINDOW_MAX_HEIGHT_RATIO).max(window_height);

        let (raw_x, raw_y) = match config.overlay_position_mode {
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
        let (x, y) = clamp_overlay_position(
            visible_frame,
            window_width,
            window_height,
            margin,
            raw_x,
            raw_y,
        );

        let frame = CGRect {
            origin: CGPoint { x, y },
            size: CGSize {
                width: window_width,
                height: window_height,
            },
        };

        // Create borderless window for modern look
        let window: Id = msg_send![ns_window, alloc];
        if window.is_null() {
            warn!("Failed to alloc NSWindow");
            return;
        }

        // Borderless + FullSizeContentView for true vibrancy effect; Resizable
        // gives decision mode a native drag affordance without changing chrome.
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
        if window.is_null() {
            warn!("Failed to init NSWindow");
            return;
        }

        // Configure window for floating overlay
        let _: () = msg_send![window, setOpaque: false];
        let clear_color: Id = msg_send![ns_color, clearColor];
        let _: () = msg_send![window, setBackgroundColor: clear_color];
        let _: () = msg_send![window, setLevel: NS_FLOATING_WINDOW_LEVEL];
        let _: () = msg_send![window, setMovableByWindowBackground: true];
        let _: () = msg_send![window, setHasShadow: true];
        let _: () = msg_send![window, setReleasedWhenClosed: false];
        let _: () = msg_send![
            window,
            setMinSize: CGSize::new(OVERLAY_WINDOW_MIN_WIDTH, OVERLAY_WINDOW_MIN_HEIGHT)
        ];
        let _: () =
            msg_send![window, setMaxSize: CGSize::new(OVERLAY_WINDOW_MAX_WIDTH, max_height)];

        // Join all spaces (follow focus)
        // Make sure the overlay shows up even when the user is in a fullscreen Space.
        let collection_behavior = NSWindowCollectionBehavior::CanJoinAllSpaces
            | NSWindowCollectionBehavior::FullScreenAuxiliary;
        let _: () = msg_send![window, setCollectionBehavior: collection_behavior];

        // Get content view
        let window_content_view: Id = msg_send![window, contentView];
        if window_content_view.is_null() {
            warn!("Failed to get content view");
            return;
        }

        // === Tahoe Liquid Glass blur ===
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
        let layer: Id = msg_send![blur_view, layer];
        if !layer.is_null() {
            let _: () = msg_send![layer, setCornerRadius: corner_radius];
            let _: () = msg_send![layer, setMasksToBounds: true];
            let border = ui_colors::overlay_sheet_border();
            let cg_border: Id = msg_send![border, CGColor];
            let _: () = msg_send![layer, setBorderColor: cg_border];
            let _: () = msg_send![layer, setBorderWidth: 1.0f64];
        }
        let _: () =
            msg_send![blur_view, setAutoresizingMask: NSVIEW_WIDTH_SIZABLE | NSVIEW_HEIGHT_SIZABLE];

        // Add blur view as background, then mount overlay controls via glass `contentView`.
        add_subview(window_content_view, blur_view);
        let content_view: Id = msg_send![Class::get("NSView").unwrap(), alloc];
        let content_view: Id = msg_send![content_view, initWithFrame: blur_frame];
        let _: () = msg_send![content_view, setAutoresizingMask: NSVIEW_WIDTH_SIZABLE | NSVIEW_HEIGHT_SIZABLE];
        let _: bool = set_glass_effect_content_view(blur_view, content_view);

        let _: () = msg_send![window, setTitle: ns_string(OVERLAY_HEADER_LABEL)];

        let padding = OVERLAY_PADDING;
        let button_height = OVERLAY_BUTTON_HEIGHT;
        let initial_layout = compute_overlay_layout_metrics(0.0, window_height, max_height);
        let header_y = initial_layout.target_height - OVERLAY_PADDING - OVERLAY_HEADER_HEIGHT;
        let info_y = header_y - OVERLAY_HEADER_GAP - OVERLAY_INFO_HEIGHT;
        let spinner_size = 14.0;
        let spinner_x = window_width - OVERLAY_PADDING - spinner_size;
        let status_gap = 6.0;
        let status_max_x = spinner_x - status_gap;
        let status_width = OVERLAY_STATUS_WIDTH.min((status_max_x - OVERLAY_PADDING).max(80.0));
        let status_x = (status_max_x - status_width).max(OVERLAY_PADDING);
        let header_width = (status_x - OVERLAY_CONTENT_GAP - OVERLAY_PADDING).max(120.0);

        let header_label = create_label(crate::ui_helpers::LabelConfig {
            frame: CGRect::new(
                &CGPoint::new(OVERLAY_PADDING, header_y),
                &CGSize::new(header_width, OVERLAY_HEADER_HEIGHT),
            ),
            text: OVERLAY_HEADER_LABEL.to_string(),
            font_size: ui_tokens::BODY_FONT_SIZE,
            bold: true,
            text_color: ui_colors::overlay_text(),
            background_color: None,
            selectable: false,
            editable: false,
        });
        let _: () = msg_send![header_label, setAutoresizingMask: NSVIEW_WIDTH_SIZABLE | NSVIEW_MIN_Y_MARGIN];
        add_subview(content_view, header_label);

        let status_field = create_label(crate::ui_helpers::LabelConfig {
            frame: CGRect::new(
                &CGPoint::new(status_x, header_y),
                &CGSize::new(status_width, OVERLAY_STATUS_HEIGHT),
            ),
            text: "Idle".to_string(),
            font_size: ui_tokens::SMALL_FONT_SIZE,
            bold: true,
            text_color: ui_colors::overlay_hint_text(),
            background_color: None,
            selectable: false,
            editable: false,
        });
        let _: () = msg_send![status_field, setAlignment: 2_isize];
        let _: () =
            msg_send![status_field, setAutoresizingMask: NSVIEW_MAX_X_MARGIN | NSVIEW_MIN_Y_MARGIN];
        add_subview(content_view, status_field);

        let auto_hide_label = create_label(crate::ui_helpers::LabelConfig {
            frame: CGRect::new(
                &CGPoint::new(OVERLAY_PADDING, info_y),
                &CGSize::new(window_width - OVERLAY_PADDING * 2.0, OVERLAY_INFO_HEIGHT),
            ),
            text: decision_hint_text(
                TranscriptionActionContractMode::Raw,
                FormatPhase::Idle,
                "",
                true,
            ),
            font_size: ui_tokens::MICRO_FONT_SIZE,
            bold: false,
            text_color: ui_colors::overlay_hint_text(),
            background_color: None,
            selectable: false,
            editable: false,
        });
        let _: () = msg_send![
            auto_hide_label,
            setAutoresizingMask: NSVIEW_WIDTH_SIZABLE | NSVIEW_MIN_Y_MARGIN
        ];
        add_subview(content_view, auto_hide_label);
        set_hidden(auto_hide_label, true);

        let spinner_frame = CGRect::new(
            &CGPoint::new(
                spinner_x,
                header_y + ((OVERLAY_HEADER_HEIGHT - spinner_size) / 2.0).max(0.0),
            ),
            &CGSize::new(spinner_size, spinner_size),
        );
        let spinner: Id = msg_send![ns_progress, alloc];
        let spinner: Id = msg_send![spinner, initWithFrame: spinner_frame];
        let _: () =
            msg_send![spinner, setAutoresizingMask: NSVIEW_MAX_X_MARGIN | NSVIEW_MIN_Y_MARGIN];
        let _: () = msg_send![spinner, setStyle: NS_PROGRESS_INDICATOR_STYLE_SPINNING];
        let _: () = msg_send![spinner, setIndeterminate: true];
        let _: () = msg_send![spinner, setDisplayedWhenStopped: false];
        add_subview(content_view, spinner);
        set_hidden(spinner, true);

        // === Scrollable text view for transcription (main area) ===
        let text_frame = CGRect::new(
            &CGPoint::new(OVERLAY_PADDING, overlay_bottom_reserved_height()),
            &CGSize::new(
                (window_width - OVERLAY_PADDING * 2.0).max(120.0),
                initial_layout.text_viewport_height,
            ),
        );
        let (text_scroll_view, text_view) = create_scrollable_text_view(text_frame, false);
        let _: () = msg_send![
            text_scroll_view,
            setAutoresizingMask: NSVIEW_WIDTH_SIZABLE | NSVIEW_HEIGHT_SIZABLE
        ];
        let ns_font_class = Class::get("NSFont").unwrap();
        let system_font: Id = msg_send![ns_font_class, systemFontOfSize: 14.0f64];
        let _: () = msg_send![text_view, setFont: system_font];
        let text_color = ui_colors::overlay_text();
        let _: () = msg_send![text_view, setTextColor: text_color];
        let _: () = msg_send![text_view, setRichText: false];
        let _: () =
            msg_send![text_view, setMinSize: CGSize::new(0.0, initial_layout.text_viewport_height)];
        let _: () = msg_send![
            text_view,
            setMaxSize: CGSize::new((window_width - OVERLAY_PADDING * 2.0).max(120.0), f64::MAX)
        ];
        let container: Id = msg_send![text_view, textContainer];
        if !container.is_null() {
            let _: () = msg_send![container, setLineFragmentPadding: 0.0f64];
        }
        set_text_view_string(text_view, "");
        add_subview(content_view, text_scroll_view);

        // Create action handler instance
        let handler_class = action_handler_class();
        let action_handler: Id = msg_send![handler_class, alloc];
        let action_handler: Id = msg_send![action_handler, init];
        let _: () = msg_send![text_view, setDelegate: action_handler];

        // Track hover on the overlay (show actions only on hover in decision mode)
        let tracking_opts = NSTRACKING_MOUSE_ENTERED_AND_EXITED
            | NSTRACKING_ACTIVE_ALWAYS
            | NSTRACKING_IN_VISIBLE_RECT;
        let tracking_area: Id = msg_send![ns_tracking_area, alloc];
        let tracking_area: Id = msg_send![
            tracking_area,
            initWithRect: blur_frame
            options: tracking_opts
            owner: action_handler
            userInfo: std::ptr::null::<Object>()
        ];
        let _: () = msg_send![content_view, addTrackingArea: tracking_area];

        // === Decision buttons (hidden during recording; show on hover) ===
        let button_width = 100.0;
        let button_gap = 10.0;
        let row_width = button_width * 3.0 + button_gap * 2.0;
        let row_x = (window_width - row_width) / 2.0;
        let commit_x = (window_width - button_width) / 2.0;

        let save_frame = CGRect {
            origin: CGPoint {
                x: row_x,
                y: padding,
            },
            size: CGSize {
                width: button_width,
                height: button_height,
            },
        };
        let copy_frame = CGRect {
            origin: CGPoint {
                x: row_x + button_width + button_gap,
                y: padding,
            },
            size: CGSize {
                width: button_width,
                height: button_height,
            },
        };
        let augment_frame = CGRect {
            origin: CGPoint {
                x: row_x + (button_width + button_gap) * 2.0,
                y: padding,
            },
            size: CGSize {
                width: button_width,
                height: button_height,
            },
        };
        let commit_frame = CGRect {
            origin: CGPoint {
                x: commit_x,
                y: padding,
            },
            size: CGSize {
                width: button_width,
                height: button_height,
            },
        };

        // Decision-mode action contract (ADR 2026-05-28 Faza 1): one hands-off
        // recording → three post-recording actions. [Format] polishes via AI + pastes,
        // [Copy] copies the transcript, [Agent] hands the whole session to Emil.
        // ROUNDED (not GLASS): Format is an active peer action alongside Copy/Agent.
        // GLASS rendered translucent → read as disabled/"wyszarzony" by the operator.
        let format_button = create_button(save_frame, "Format", button_style::ROUNDED);
        let copy_button = create_button(copy_frame, "Copy", button_style::ROUNDED);
        let agent_button = create_button(augment_frame, "Agent", button_style::ROUNDED);
        let commit_button = create_button(commit_frame, "Finish", button_style::GLASS);
        let button_autoresize = NSVIEW_MAX_Y_MARGIN;
        let _: () = msg_send![format_button, setAutoresizingMask: button_autoresize];
        let _: () = msg_send![copy_button, setAutoresizingMask: button_autoresize];
        let _: () = msg_send![agent_button, setAutoresizingMask: button_autoresize];
        let _: () = msg_send![commit_button, setAutoresizingMask: button_autoresize];
        set_tooltip(
            copy_button,
            copy_action_tooltip(TranscriptionActionContractMode::Raw),
        );
        set_tooltip(
            agent_button,
            augment_action_tooltip(TranscriptionActionContractMode::Raw, FormatPhase::Idle),
        );
        set_tooltip(
            format_button,
            "Format the transcript with AI in the overlay",
        );
        set_tooltip(commit_button, "Stop recording and enter decision mode");

        button_set_action(
            format_button,
            action_handler,
            overlay_button_selector(OverlayActionButtonRole::FormatPaste, FormatPhase::Idle),
        );
        button_set_action(
            copy_button,
            action_handler,
            overlay_button_selector(OverlayActionButtonRole::Copy, FormatPhase::Idle),
        );
        button_set_action(
            agent_button,
            action_handler,
            overlay_button_selector(OverlayActionButtonRole::AgentClose, FormatPhase::Idle),
        );
        button_set_action(
            commit_button,
            action_handler,
            overlay_button_selector(OverlayActionButtonRole::Finish, FormatPhase::Idle),
        );

        add_subview(content_view, format_button);
        add_subview(content_view, copy_button);
        add_subview(content_view, agent_button);
        add_subview(content_view, commit_button);

        set_hidden(format_button, true);
        set_hidden(copy_button, true);
        set_hidden(agent_button, true);
        set_hidden(commit_button, true);

        // Show the window with fade-in animation
        window_set_alpha(window, 0.0);
        window_show(window);
        animate_fade(window, 1.0, 0.2);

        // Re-acquire lock to store widget pointers (quick write, no AppKit calls).
        let mut state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
        // Guard: if another path filled window while we were creating, abandon ours.
        if state.window.is_some() {
            drop(state);
            warn!("Overlay window created concurrently; discarding duplicate");
            window_discard(window);
            return;
        }
        state.window = Some(window as usize);
        state.header_label = Some(header_label as usize);
        state.text_scroll_view = Some(text_scroll_view as usize);
        state.text_view = Some(text_view as usize);
        state.status_field = Some(status_field as usize);
        state.auto_hide_label = Some(auto_hide_label as usize);
        state.blur_view = Some(blur_view as usize);
        state.copy_button = Some(copy_button as usize);
        state.augment_button = Some(agent_button as usize);
        state.save_button = Some(format_button as usize);
        state.commit_button = Some(commit_button as usize);
        state.progress_indicator = Some(spinner as usize);
        state.tracking_area = Some(tracking_area as usize);
        state.decision_mode = false;
        state.hover_active = false;
        state.action_handler = Some(action_handler as usize);
        state.min_height = window_height;
        state.max_height = max_height;
        state.last_applied_height = window_height;
        state.last_layout_resize_at = Instant::now();
        state.pending_layout_resize = false;

        // DEADLOCK PREVENTION: snapshot + drop before AppKit layout calls.
        let snap = OverlaySnapshot::from_state(&state);
        drop(state);

        refresh_action_contract_ui_unlocked(&snap, TranscriptionActionContractMode::Raw, false);
        reset_overlay_to_idle_unlocked(&snap);
        let new_h = resize_overlay_unlocked(&snap);
        {
            let mut state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
            state.last_applied_height = new_h;
            state.last_layout_resize_at = Instant::now();
            state.pending_layout_resize = false;
        }

        info!("Transcription overlay shown (Tahoe-style with HudWindow vibrancy)");
    }
}
