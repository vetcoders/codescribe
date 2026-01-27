//! Simple transcription overlay for non-assistive modes.
//!
//! This module provides a minimal floating overlay window that:
//! - Shows status during recording (Recording..., Processing...)
//! - Displays live streaming transcription text
//! - Result goes to clipboard (no chat, no conversation)
//! - Auto-hides after recording completion
//!
//! Use this for: Ctrl hold (raw), Left ⌥⌥ toggle (normal)
//! For assistive modes (Ctrl+Shift, Right ⌥⌥), use voice_chat_ui instead.
//!
//! Design: macOS Tahoe-style with NSVisualEffectView (HudWindow material)

// Allow unexpected cfgs from objc crate's msg_send! macro
// Allow unused API methods - they're part of the public interface for future use

use crate::os::clipboard;
use codescribe_core::config::{Config, OverlayPositionMode};
use core_graphics::geometry::{CGPoint, CGRect, CGSize};
use dispatch::Queue;
use objc::runtime::{Class, Object};
use objc::{msg_send, sel, sel_impl};
use objc2_app_kit::{
    NSBackingStoreType, NSVisualEffectBlendingMode, NSVisualEffectMaterial, NSVisualEffectState,
    NSWindowCollectionBehavior, NSWindowStyleMask,
};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;
use tracing::{debug, info, warn};

use crate::ui_helpers::{
    add_subview, animate_fade, button_set_action, button_style, clamp_overlay_position,
    color_white, create_button, set_hidden, set_text, window_close, window_set_alpha, window_show,
};
use objc::declare::ClassDecl;
use objc::runtime::Sel;
use std::sync::Once;

// Type alias for Objective-C object pointers
type Id = *mut Object;

// Window level constants
const NS_FLOATING_WINDOW_LEVEL: i64 = 3;
const NS_PROGRESS_INDICATOR_STYLE_SPINNING: i64 = 1;
const NSTRACKING_MOUSE_ENTERED_AND_EXITED: u64 = 1 << 0;
const NSTRACKING_ACTIVE_ALWAYS: u64 = 1 << 7;
const NSTRACKING_IN_VISIBLE_RECT: u64 = 1 << 9;

const OVERLAY_WINDOW_WIDTH: f64 = 420.0;
const OVERLAY_WINDOW_MIN_HEIGHT: f64 = 180.0;
const OVERLAY_WINDOW_MAX_HEIGHT_RATIO: f64 = 0.5;
const OVERLAY_PADDING: f64 = 16.0;
const OVERLAY_STATUS_HEIGHT: f64 = 28.0;
const OVERLAY_BUTTON_HEIGHT: f64 = 28.0;
const OVERLAY_BUTTON_MARGIN: f64 = 8.0;
const OVERLAY_CORNER_RADIUS: f64 = 16.0;
const MAX_TAIL_LINES: usize = 16;
const MAX_TAIL_CHARS: usize = 2000;

// Auto-hide delay after recording completes
const AUTO_HIDE_DELAY_SECS: u64 = 5;

/// Configuration for the transcription overlay
#[derive(Debug, Clone)]
pub struct TranscriptionOverlayConfig {
    /// Width of the overlay window in pixels
    pub width: f64,
    /// Height of the overlay window in pixels
    pub height: f64,
}

impl Default for TranscriptionOverlayConfig {
    fn default() -> Self {
        Self {
            width: 420.0,
            height: 180.0,
        }
    }
}

/// Transcription overlay state
struct TranscriptionOverlayState {
    window: Option<usize>,
    text_field: Option<usize>,
    status_field: Option<usize>,
    blur_view: Option<usize>,
    copy_button: Option<usize>,
    augment_button: Option<usize>,
    archive_button: Option<usize>,
    commit_button: Option<usize>,
    progress_indicator: Option<usize>,
    tracking_area: Option<usize>,
    decision_mode: bool,
    hover_active: bool,
    action_handler: Option<usize>,
    accumulated_text: String,
    window_width: f64,
    min_height: f64,
    max_height: f64,
}

lazy_static::lazy_static! {
    static ref OVERLAY_STATE: Mutex<TranscriptionOverlayState> = Mutex::new(TranscriptionOverlayState {
        window: None,
        text_field: None,
        status_field: None,
        blur_view: None,
        copy_button: None,
        augment_button: None,
        archive_button: None,
        commit_button: None,
        progress_indicator: None,
        tracking_area: None,
        decision_mode: false,
        hover_active: false,
        action_handler: None,
        accumulated_text: String::new(),
        window_width: OVERLAY_WINDOW_WIDTH,
        min_height: OVERLAY_WINDOW_MIN_HEIGHT,
        max_height: OVERLAY_WINDOW_MIN_HEIGHT,
    });
}

/// Flag to track if auto-hide timer is pending
static AUTO_HIDE_PENDING: AtomicBool = AtomicBool::new(false);
/// Counter to invalidate old timers
static AUTO_HIDE_GENERATION: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

// ═══════════════════════════════════════════════════════════
// Action Handler Class (for button callbacks)
// ═══════════════════════════════════════════════════════════

static ACTION_HANDLER_INIT: Once = Once::new();
static mut ACTION_HANDLER_CLASS: *const Class = std::ptr::null();

fn action_handler_class() -> *const Class {
    ACTION_HANDLER_INIT.call_once(|| unsafe {
        let superclass = Class::get("NSObject").unwrap();
        let mut decl = ClassDecl::new("TranscriptionOverlayActionHandler", superclass).unwrap();

        decl.add_method(
            sel!(onCopyTranscript:),
            on_copy_transcript as extern "C" fn(&Object, Sel, Id),
        );
        decl.add_method(
            sel!(onAugmentTranscript:),
            on_augment_transcript as extern "C" fn(&Object, Sel, Id),
        );
        decl.add_method(
            sel!(onArchiveTranscript:),
            on_archive_transcript as extern "C" fn(&Object, Sel, Id),
        );
        decl.add_method(
            sel!(onCommitRecording:),
            on_commit_recording as extern "C" fn(&Object, Sel, Id),
        );
        decl.add_method(
            sel!(mouseEntered:),
            on_mouse_entered as extern "C" fn(&Object, Sel, Id),
        );
        decl.add_method(
            sel!(mouseExited:),
            on_mouse_exited as extern "C" fn(&Object, Sel, Id),
        );

        ACTION_HANDLER_CLASS = decl.register();
    });
    unsafe { ACTION_HANDLER_CLASS }
}

/// Handler: Copy (AI format) to clipboard
extern "C" fn on_copy_transcript(_this: &Object, _cmd: Sel, _sender: Id) {
    let text = get_transcription_text();
    if text.is_empty() {
        return;
    }
    run_ai_copy(text, false);
}

/// Handler: Augment (AI assist) to clipboard
extern "C" fn on_augment_transcript(_this: &Object, _cmd: Sel, _sender: Id) {
    let text = get_transcription_text();
    if text.is_empty() {
        return;
    }
    run_ai_copy(text, true);
}

/// Handler: Archive (no-op save already happened; just close overlay)
extern "C" fn on_archive_transcript(_this: &Object, _cmd: Sel, _sender: Id) {
    hide_transcription_overlay();
}

/// Handler: Commit recording (stop stream + enter decision mode)
extern "C" fn on_commit_recording(_this: &Object, _cmd: Sel, _sender: Id) {
    crate::controller::request_recording_commit();
}

extern "C" fn on_mouse_entered(_this: &Object, _cmd: Sel, _sender: Id) {
    let mut state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
    state.hover_active = true;
    if state.decision_mode {
        set_action_buttons_visible(&state, true);
    }
}

extern "C" fn on_mouse_exited(_this: &Object, _cmd: Sel, _sender: Id) {
    let mut state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
    state.hover_active = false;
    set_action_buttons_visible(&state, false);
}

fn set_action_buttons_visible(state: &TranscriptionOverlayState, visible: bool) {
    if let Some(copy_ptr) = state.copy_button {
        unsafe {
            set_hidden(copy_ptr as Id, !visible);
        }
    }
    if let Some(augment_ptr) = state.augment_button {
        unsafe {
            set_hidden(augment_ptr as Id, !visible);
        }
    }
    if let Some(archive_ptr) = state.archive_button {
        unsafe {
            set_hidden(archive_ptr as Id, !visible);
        }
    }
}

fn set_recording_button_visible(state: &TranscriptionOverlayState, visible: bool) {
    if let Some(commit_ptr) = state.commit_button {
        unsafe {
            set_hidden(commit_ptr as Id, !visible);
        }
    }
}

fn set_buttons_enabled(state: &TranscriptionOverlayState, enabled: bool) {
    if let Some(copy_ptr) = state.copy_button {
        unsafe {
            crate::ui_helpers::set_enabled(copy_ptr as Id, enabled);
        }
    }
    if let Some(augment_ptr) = state.augment_button {
        unsafe {
            crate::ui_helpers::set_enabled(augment_ptr as Id, enabled);
        }
    }
    if let Some(archive_ptr) = state.archive_button {
        unsafe {
            crate::ui_helpers::set_enabled(archive_ptr as Id, enabled);
        }
    }
}

fn set_status_message(state: &TranscriptionOverlayState, msg: &str, show: bool) {
    if let Some(status_ptr) = state.status_field {
        unsafe {
            set_text(status_ptr as Id, msg);
            set_hidden(status_ptr as Id, !show);
        }
    }
    if let Some(spinner_ptr) = state.progress_indicator {
        unsafe {
            set_hidden(spinner_ptr as Id, !show);
        }
        if show {
            unsafe {
                let _: () =
                    msg_send![spinner_ptr as Id, startAnimation: std::ptr::null::<Object>()];
            }
        } else {
            unsafe {
                let _: () = msg_send![spinner_ptr as Id, stopAnimation: std::ptr::null::<Object>()];
            }
        }
    }
}

fn run_ai_copy(text: String, augment: bool) {
    let label = if augment {
        "Augmentowanie…"
    } else {
        "Formatowanie…"
    };
    {
        let state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
        set_status_message(&state, label, true);
        set_buttons_enabled(&state, false);
    }
    let lang = Config::load().whisper_language.as_str();
    tokio::spawn(async move {
        let result =
            crate::ai_formatting::format_text_with_status(&text, Some(lang), augment, None).await;

        Queue::main().exec_async(move || {
            let state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
            match result.status {
                crate::ai_formatting::AiFormatStatus::Applied => {
                    if let Err(e) = clipboard::set_clipboard(&result.text) {
                        warn!("Failed to copy formatted text: {}", e);
                        set_status_message(&state, "Błąd kopiowania", true);
                        set_buttons_enabled(&state, true);
                    } else {
                        info!("Copied formatted transcript ({} chars)", result.text.len());
                        hide_transcription_overlay();
                    }
                }
                crate::ai_formatting::AiFormatStatus::Failed => {
                    set_status_message(&state, "AI Failed", true);
                    set_buttons_enabled(&state, true);
                }
                crate::ai_formatting::AiFormatStatus::Skipped => {
                    set_status_message(&state, "AI Skipped", true);
                    set_buttons_enabled(&state, true);
                }
            }
        });
    });
}

fn measure_text_height(text_field: Id, width: f64) -> f64 {
    unsafe {
        let cell: Id = msg_send![text_field, cell];
        if cell.is_null() {
            return 0.0;
        }
        let bounds = CGRect {
            origin: CGPoint { x: 0.0, y: 0.0 },
            size: CGSize {
                width,
                height: f64::MAX,
            },
        };
        let size: CGSize = msg_send![cell, cellSizeForBounds: bounds];
        size.height
    }
}

fn trim_text_to_tail(text: &mut String) {
    if text.is_empty() {
        return;
    }

    let mut trimmed = false;
    let lines: Vec<&str> = text.lines().collect();
    let mut tail_text = if lines.len() > MAX_TAIL_LINES {
        trimmed = true;
        lines[lines.len() - MAX_TAIL_LINES..].join("\n")
    } else {
        text.clone()
    };

    if tail_text.chars().count() > MAX_TAIL_CHARS {
        trimmed = true;
        let tail_chars: String = tail_text
            .chars()
            .rev()
            .take(MAX_TAIL_CHARS)
            .collect::<Vec<char>>()
            .into_iter()
            .rev()
            .collect();
        tail_text = tail_chars;
    }

    if trimmed {
        *text = tail_text;
    }
}

fn resize_overlay_to_fit_text(state: &mut TranscriptionOverlayState) {
    let (window_ptr, text_field_ptr) = match (state.window, state.text_field) {
        (Some(window_ptr), Some(text_field_ptr)) => (window_ptr as Id, text_field_ptr as Id),
        _ => return,
    };

    let text_width = state.window_width - OVERLAY_PADDING * 2.0;
    let mut text_height = measure_text_height(text_field_ptr, text_width);
    let mut required_window_height =
        text_height + OVERLAY_PADDING * 3.0 + OVERLAY_BUTTON_HEIGHT + OVERLAY_BUTTON_MARGIN;

    if required_window_height > state.max_height {
        trim_text_to_tail(&mut state.accumulated_text);
        unsafe {
            set_text(text_field_ptr, &state.accumulated_text);
        }
        text_height = measure_text_height(text_field_ptr, text_width);
        required_window_height =
            text_height + OVERLAY_PADDING * 3.0 + OVERLAY_BUTTON_HEIGHT + OVERLAY_BUTTON_MARGIN;
    }

    let target_height = required_window_height
        .max(state.min_height)
        .min(state.max_height);

    unsafe {
        let current_frame: CGRect = msg_send![window_ptr, frame];
        let top_y = current_frame.origin.y + current_frame.size.height;
        let new_frame = CGRect {
            origin: CGPoint {
                x: current_frame.origin.x,
                y: top_y - target_height,
            },
            size: CGSize {
                width: state.window_width,
                height: target_height,
            },
        };
        let _: () = msg_send![window_ptr, setFrame: new_frame display: true];
        let _: () = msg_send![window_ptr, setLevel: NS_FLOATING_WINDOW_LEVEL];

        let text_frame = CGRect {
            origin: CGPoint {
                x: OVERLAY_PADDING,
                y: OVERLAY_PADDING + OVERLAY_BUTTON_HEIGHT + OVERLAY_BUTTON_MARGIN,
            },
            size: CGSize {
                width: state.window_width - OVERLAY_PADDING * 2.0,
                height: target_height
                    - OVERLAY_PADDING * 3.0
                    - OVERLAY_BUTTON_HEIGHT
                    - OVERLAY_BUTTON_MARGIN,
            },
        };
        let _: () = msg_send![text_field_ptr, setFrame: text_frame];

        if let Some(status_ptr) = state.status_field {
            let status_frame = CGRect {
                origin: CGPoint {
                    x: OVERLAY_PADDING,
                    y: target_height - OVERLAY_STATUS_HEIGHT - OVERLAY_PADDING,
                },
                size: CGSize {
                    width: state.window_width - OVERLAY_PADDING * 2.0,
                    height: OVERLAY_STATUS_HEIGHT,
                },
            };
            let _: () = msg_send![status_ptr as Id, setFrame: status_frame];
        }

        if let Some(spinner_ptr) = state.progress_indicator {
            let spinner_size = 14.0;
            let spinner_frame = CGRect {
                origin: CGPoint {
                    x: state.window_width - OVERLAY_PADDING - spinner_size,
                    y: target_height - OVERLAY_STATUS_HEIGHT - OVERLAY_PADDING + 7.0,
                },
                size: CGSize {
                    width: spinner_size,
                    height: spinner_size,
                },
            };
            let _: () = msg_send![spinner_ptr as Id, setFrame: spinner_frame];
        }

        if let Some(blur_ptr) = state.blur_view {
            let blur_frame = CGRect {
                origin: CGPoint { x: 0.0, y: 0.0 },
                size: CGSize {
                    width: state.window_width,
                    height: target_height,
                },
            };
            let _: () = msg_send![blur_ptr as Id, setFrame: blur_frame];
        }

        let button_width = 100.0;
        let button_gap = 10.0;
        let row_width = button_width * 3.0 + button_gap * 2.0;
        let row_x = (state.window_width - row_width) / 2.0;
        let archive_frame = CGRect {
            origin: CGPoint {
                x: row_x,
                y: OVERLAY_PADDING,
            },
            size: CGSize {
                width: button_width,
                height: OVERLAY_BUTTON_HEIGHT,
            },
        };
        let copy_frame = CGRect {
            origin: CGPoint {
                x: row_x + button_width + button_gap,
                y: OVERLAY_PADDING,
            },
            size: CGSize {
                width: button_width,
                height: OVERLAY_BUTTON_HEIGHT,
            },
        };
        let augment_frame = CGRect {
            origin: CGPoint {
                x: row_x + (button_width + button_gap) * 2.0,
                y: OVERLAY_PADDING,
            },
            size: CGSize {
                width: button_width,
                height: OVERLAY_BUTTON_HEIGHT,
            },
        };

        if let Some(archive_ptr) = state.archive_button {
            let _: () = msg_send![archive_ptr as Id, setFrame: archive_frame];
        }
        if let Some(copy_ptr) = state.copy_button {
            let _: () = msg_send![copy_ptr as Id, setFrame: copy_frame];
        }
        if let Some(augment_ptr) = state.augment_button {
            let _: () = msg_send![augment_ptr as Id, setFrame: augment_frame];
        }
    }
}

fn update_overlay_text_and_layout(state: &mut TranscriptionOverlayState) {
    if let Some(text_field_ptr) = state.text_field {
        unsafe {
            set_text(text_field_ptr as Id, &state.accumulated_text);
        }
        resize_overlay_to_fit_text(state);
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
            let window = window_ptr as Id;
            let _: () = msg_send![window, setLevel: NS_FLOATING_WINDOW_LEVEL];
            window_show(window);
            resize_overlay_to_fit_text(&mut state);
            info!("Transcription overlay reused");
            return;
        }

        state.accumulated_text.clear();

        // Get classes
        let ns_window_class = Class::get("NSWindow");
        let ns_text_field_class = Class::get("NSTextField");
        let ns_screen_class = Class::get("NSScreen");
        let ns_string_class = Class::get("NSString");
        let ns_visual_effect_view_class = Class::get("NSVisualEffectView");
        let ns_color_class = Class::get("NSColor");
        let ns_progress_class = Class::get("NSProgressIndicator");
        let ns_tracking_area_class = Class::get("NSTrackingArea");

        // Defensive checks for Cocoa classes
        if ns_window_class.is_none()
            || ns_text_field_class.is_none()
            || ns_screen_class.is_none()
            || ns_string_class.is_none()
            || ns_visual_effect_view_class.is_none()
            || ns_color_class.is_none()
            || ns_progress_class.is_none()
            || ns_tracking_area_class.is_none()
        {
            warn!("Failed to get required Cocoa classes");
            return;
        }

        let ns_window = ns_window_class.unwrap();
        let ns_text_field = ns_text_field_class.unwrap();
        let ns_screen = ns_screen_class.unwrap();
        let ns_string = ns_string_class.unwrap();
        let ns_visual_effect_view = ns_visual_effect_view_class.unwrap();
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
        let corner_radius = OVERLAY_CORNER_RADIUS;
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

        // Borderless + FullSizeContentView for true vibrancy effect
        let style_mask = NSWindowStyleMask::Borderless | NSWindowStyleMask::FullSizeContentView;
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

        // Join all spaces (follow focus)
        // Make sure the overlay shows up even when the user is in a fullscreen Space.
        let collection_behavior = NSWindowCollectionBehavior::CanJoinAllSpaces
            | NSWindowCollectionBehavior::FullScreenAuxiliary;
        let _: () = msg_send![window, setCollectionBehavior: collection_behavior];

        // Get content view
        let content_view: Id = msg_send![window, contentView];
        if content_view.is_null() {
            warn!("Failed to get content view");
            return;
        }

        // === NSVisualEffectView for macOS Tahoe-style blur ===
        let blur_frame = CGRect {
            origin: CGPoint { x: 0.0, y: 0.0 },
            size: CGSize {
                width: window_width,
                height: window_height,
            },
        };

        let blur_view: Id = msg_send![ns_visual_effect_view, alloc];
        let blur_view: Id = msg_send![blur_view, initWithFrame: blur_frame];

        // HudWindow material (13) - perfect for floating overlays
        let material = NSVisualEffectMaterial::HUDWindow;
        let _: () = msg_send![blur_view, setMaterial: material];

        // BehindWindow blending for true vibrancy
        let blending = NSVisualEffectBlendingMode::BehindWindow;
        let _: () = msg_send![blur_view, setBlendingMode: blending];

        // Always active (don't dim when window loses focus)
        let effect_state = NSVisualEffectState::Active;
        let _: () = msg_send![blur_view, setState: effect_state];

        // Enable layer-backed view for corner radius
        let _: () = msg_send![blur_view, setWantsLayer: true];

        // Set corner radius on the layer
        let layer: Id = msg_send![blur_view, layer];
        if !layer.is_null() {
            let _: () = msg_send![layer, setCornerRadius: corner_radius];
            let _: () = msg_send![layer, setMasksToBounds: true];
        }

        // Add blur view as background
        add_subview(content_view, blur_view);

        // === Status indicator (top) === (hidden by default)
        let status_height = OVERLAY_STATUS_HEIGHT;
        let padding = OVERLAY_PADDING;
        let status_frame = CGRect {
            origin: CGPoint {
                x: padding,
                y: window_height - status_height - padding,
            },
            size: CGSize {
                width: window_width - padding * 2.0,
                height: status_height,
            },
        };

        let status_field: Id = msg_send![ns_text_field, alloc];
        let status_field: Id = msg_send![status_field, initWithFrame: status_frame];
        let _: () = msg_send![status_field, setBezeled: false];
        let _: () = msg_send![status_field, setDrawsBackground: false];
        let _: () = msg_send![status_field, setEditable: false];
        let _: () = msg_send![status_field, setSelectable: false];

        // White text for contrast on dark blur
        let white_color: Id = msg_send![ns_color, whiteColor];
        let _: () = msg_send![status_field, setTextColor: white_color];

        // Bold system font for status
        let ns_font_class = Class::get("NSFont").unwrap();
        let bold_font: Id = msg_send![ns_font_class, boldSystemFontOfSize: 13.0f64];
        let _: () = msg_send![status_field, setFont: bold_font];

        // Recording indicator with emoji
        let initial_status: Id = msg_send![ns_string, stringWithUTF8String: c"".as_ptr()];
        let _: () = msg_send![status_field, setStringValue: initial_status];

        add_subview(content_view, status_field);
        set_hidden(status_field, true);

        // Spinner (hidden by default)
        let spinner_size = 14.0;
        let spinner_frame = CGRect {
            origin: CGPoint {
                x: window_width - padding - spinner_size,
                y: window_height - status_height - padding + 7.0,
            },
            size: CGSize {
                width: spinner_size,
                height: spinner_size,
            },
        };
        let spinner: Id = msg_send![ns_progress, alloc];
        let spinner: Id = msg_send![spinner, initWithFrame: spinner_frame];
        let _: () = msg_send![spinner, setStyle: NS_PROGRESS_INDICATOR_STYLE_SPINNING];
        let _: () = msg_send![spinner, setIndeterminate: true];
        let _: () = msg_send![spinner, setDisplayedWhenStopped: false];
        add_subview(content_view, spinner);
        set_hidden(spinner, true);

        // === Text field for transcription (main area) ===
        let button_height = OVERLAY_BUTTON_HEIGHT;
        let button_margin = OVERLAY_BUTTON_MARGIN;
        let text_frame = CGRect {
            origin: CGPoint {
                x: padding,
                y: padding + button_height + button_margin,
            },
            size: CGSize {
                width: window_width - padding * 2.0,
                height: window_height - padding * 3.0 - button_height - button_margin,
            },
        };

        let text_field: Id = msg_send![ns_text_field, alloc];
        let text_field: Id = msg_send![text_field, initWithFrame: text_frame];
        let _: () = msg_send![text_field, setBezeled: false];
        let _: () = msg_send![text_field, setDrawsBackground: false];
        let _: () = msg_send![text_field, setEditable: false];
        let _: () = msg_send![text_field, setSelectable: true];

        // Semi-transparent white for text
        let text_color = color_white(0.9);
        let _: () = msg_send![text_field, setTextColor: text_color];

        // System font for content
        let system_font: Id = msg_send![ns_font_class, systemFontOfSize: 14.0f64];
        let _: () = msg_send![text_field, setFont: system_font];

        // Enable word wrapping
        let cell: Id = msg_send![text_field, cell];
        if !cell.is_null() {
            let _: () = msg_send![cell, setWraps: true];
            let _: () = msg_send![cell, setScrollable: false];
            // Top-left alignment
            let _: () = msg_send![cell, setUsesSingleLineMode: false];
        }

        let empty_str: Id = msg_send![ns_string, stringWithUTF8String: c"".as_ptr()];
        let _: () = msg_send![text_field, setStringValue: empty_str];

        add_subview(content_view, text_field);

        // Create action handler instance
        let handler_class = action_handler_class();
        let action_handler: Id = msg_send![handler_class, alloc];
        let action_handler: Id = msg_send![action_handler, init];

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

        let archive_frame = CGRect {
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

        let archive_button = create_button(archive_frame, "Archiwizuj", button_style::ROUNDED);
        let copy_button = create_button(copy_frame, "Kopiuj", button_style::ROUNDED);
        let augment_button = create_button(augment_frame, "Augmentuj", button_style::ROUNDED);
        let commit_button = create_button(commit_frame, "Zakończ", button_style::ROUNDED);

        button_set_action(archive_button, action_handler, sel!(onArchiveTranscript:));
        button_set_action(copy_button, action_handler, sel!(onCopyTranscript:));
        button_set_action(augment_button, action_handler, sel!(onAugmentTranscript:));
        button_set_action(commit_button, action_handler, sel!(onCommitRecording:));

        add_subview(content_view, archive_button);
        add_subview(content_view, copy_button);
        add_subview(content_view, augment_button);
        add_subview(content_view, commit_button);

        set_hidden(archive_button, true);
        set_hidden(copy_button, true);
        set_hidden(augment_button, true);
        set_hidden(commit_button, true);

        // Show the window with fade-in animation
        window_set_alpha(window, 0.0);
        window_show(window);
        animate_fade(window, 1.0, 0.2);

        state.window = Some(window as usize);
        state.text_field = Some(text_field as usize);
        state.status_field = Some(status_field as usize);
        state.blur_view = Some(blur_view as usize);
        state.copy_button = Some(copy_button as usize);
        state.augment_button = Some(augment_button as usize);
        state.archive_button = Some(archive_button as usize);
        state.commit_button = Some(commit_button as usize);
        state.progress_indicator = Some(spinner as usize);
        state.tracking_area = Some(tracking_area as usize);
        state.decision_mode = false;
        state.hover_active = false;
        state.action_handler = Some(action_handler as usize);
        state.window_width = window_width;
        state.min_height = window_height;
        state.max_height = max_height;

        info!("Transcription overlay shown (Tahoe-style with HudWindow vibrancy)");
    }
}

/// Update the status text in the overlay
pub fn update_transcription_status(status: &str) {
    let status_owned = status.to_string();
    Queue::main().exec_async(move || {
        update_transcription_status_impl(&status_owned);
    });
}

fn update_transcription_status_impl(status: &str) {
    let state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(status_field_ptr) = state.status_field {
        unsafe {
            set_text(status_field_ptr as Id, status);
        }
    }
}

/// Append a delta (streaming token) to the overlay text
pub fn append_transcription_delta(delta: &str) {
    let delta_owned = delta.to_string();
    Queue::main().exec_async(move || {
        append_transcription_delta_impl(&delta_owned);
    });
}

fn append_transcription_delta_impl(delta: &str) {
    let mut state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
    apply_delta_with_backspace(&mut state.accumulated_text, delta);
    update_overlay_text_and_layout(&mut state);
}

fn apply_delta_with_backspace(target: &mut String, delta: &str) {
    for ch in delta.chars() {
        if ch == '\u{0008}' {
            target.pop();
        } else {
            target.push(ch);
        }
    }
}

/// Set the full text in the overlay
pub fn set_transcription_text(text: &str) {
    let text_owned = text.to_string();
    Queue::main().exec_async(move || {
        set_transcription_text_impl(&text_owned);
    });
}

fn set_transcription_text_impl(text: &str) {
    let mut state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
    state.accumulated_text = text.to_string();
    update_overlay_text_and_layout(&mut state);
}

/// Get the accumulated text from the overlay
pub fn get_transcription_text() -> String {
    let state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
    state.accumulated_text.clone()
}

/// Clear the text content of the overlay
pub fn clear_transcription_text() {
    Queue::main().exec_async(|| {
        clear_transcription_text_impl();
    });
}

fn clear_transcription_text_impl() {
    let mut state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
    state.accumulated_text.clear();

    if let Some(text_field_ptr) = state.text_field {
        unsafe {
            set_text(text_field_ptr as Id, "");
        }
    }
    resize_overlay_to_fit_text(&mut state);
    if let Some(copy_ptr) = state.copy_button {
        unsafe {
            set_hidden(copy_ptr as Id, true);
        }
    }
    if let Some(augment_ptr) = state.augment_button {
        unsafe {
            set_hidden(augment_ptr as Id, true);
        }
    }
    if let Some(archive_ptr) = state.archive_button {
        unsafe {
            set_hidden(archive_ptr as Id, true);
        }
    }
    if let Some(commit_ptr) = state.commit_button {
        unsafe {
            set_hidden(commit_ptr as Id, true);
        }
    }
    if let Some(status_ptr) = state.status_field {
        unsafe {
            set_hidden(status_ptr as Id, true);
        }
    }
    if let Some(spinner_ptr) = state.progress_indicator {
        unsafe {
            set_hidden(spinner_ptr as Id, true);
        }
    }
    state.decision_mode = false;
    state.hover_active = false;
}

/// Check if the transcription overlay is currently visible
pub fn is_transcription_overlay_visible() -> bool {
    let state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
    state.window.is_some()
}

/// Schedule auto-hide after delay (call this when recording finishes)
pub fn schedule_auto_hide() {
    let generation = AUTO_HIDE_GENERATION.fetch_add(1, Ordering::SeqCst) + 1;
    AUTO_HIDE_PENDING.store(true, Ordering::SeqCst);

    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_secs(AUTO_HIDE_DELAY_SECS));

        if should_auto_hide(generation) {
            hide_transcription_overlay();
            debug!(
                "Transcription overlay auto-hidden after {}s",
                AUTO_HIDE_DELAY_SECS
            );
        } else {
            debug!("Auto-hide skipped");
        }
    });
}

fn should_auto_hide(expected_generation: u64) -> bool {
    if AUTO_HIDE_GENERATION.load(Ordering::SeqCst) != expected_generation
        || !AUTO_HIDE_PENDING.load(Ordering::SeqCst)
    {
        return false;
    }

    let hovered = {
        let state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
        state.hover_active
    };

    !hovered
}

/// Enter decision mode: show actions on hover for the current transcript
pub fn enter_decision_mode() {
    Queue::main().exec_async(|| {
        let mut state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
        state.decision_mode = true;
        let show = state.hover_active;
        set_action_buttons_visible(&state, show);
        set_recording_button_visible(&state, false);
    });
}

/// Enter recording mode: hide actions and status
pub fn enter_recording_mode() {
    Queue::main().exec_async(|| {
        let mut state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
        state.decision_mode = false;
        state.hover_active = false;
        set_action_buttons_visible(&state, false);
        set_recording_button_visible(&state, true);
        set_status_message(&state, "", false);
    });
}

/// Hide the transcription overlay window (with fade-out animation)
pub fn hide_transcription_overlay() {
    // Cancel any pending auto-hide
    AUTO_HIDE_PENDING.store(false, Ordering::SeqCst);

    Queue::main().exec_async(|| {
        hide_transcription_overlay_impl();
    });
}

/// Closes a window by raw pointer (used for delayed close after animation)
fn close_window_by_ptr(window_ptr: usize) {
    unsafe {
        window_close(window_ptr as Id);
    }
}

fn hide_transcription_overlay_impl() {
    let mut state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(window_ptr) = state.window.take() {
        let window = window_ptr as Id;

        // Fade out animation (0.15s)
        unsafe {
            animate_fade(window, 0.0, 0.15);
        }

        // Close window after brief delay for animation
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(200));
            Queue::main().exec_async(move || {
                close_window_by_ptr(window_ptr);
            });
        });

        debug!("Transcription overlay hidden");
    }
    state.text_field = None;
    state.status_field = None;
    state.blur_view = None;
    state.copy_button = None;
    state.augment_button = None;
    state.archive_button = None;
    state.commit_button = None;
    state.progress_indicator = None;
    state.tracking_area = None;
    state.decision_mode = false;
    state.hover_active = false;
    state.action_handler = None;
    // Note: accumulated_text is NOT cleared here - it's needed for clipboard copy
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_transcription_text() {
        // Just verify the function doesn't panic
        let _ = get_transcription_text();
    }

    #[test]
    fn test_overlay_config_default() {
        let config = TranscriptionOverlayConfig::default();
        assert_eq!(config.width, 420.0);
        assert_eq!(config.height, 180.0);
    }

    #[test]
    fn test_is_overlay_visible_returns_bool() {
        // Just verify the function returns a bool without panic
        let visible = is_transcription_overlay_visible();
        let _ = visible;
    }

    #[test]
    fn test_auto_hide_generation() {
        // Test that generation counter increments
        let gen1 = AUTO_HIDE_GENERATION.load(Ordering::SeqCst);
        AUTO_HIDE_GENERATION.fetch_add(1, Ordering::SeqCst);
        let gen2 = AUTO_HIDE_GENERATION.load(Ordering::SeqCst);
        assert_eq!(gen2, gen1 + 1);
    }

    #[test]
    fn test_auto_hide_delay_seconds() {
        assert_eq!(AUTO_HIDE_DELAY_SECS, 5);
    }

    #[test]
    fn test_auto_hide_hover_guard() {
        AUTO_HIDE_GENERATION.store(42, Ordering::SeqCst);
        AUTO_HIDE_PENDING.store(true, Ordering::SeqCst);

        {
            let mut state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
            state.hover_active = true;
        }
        assert!(!should_auto_hide(42));

        {
            let mut state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
            state.hover_active = false;
        }
        assert!(should_auto_hide(42));
    }
}
