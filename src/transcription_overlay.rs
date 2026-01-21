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
#![allow(unexpected_cfgs)]
// Allow unused API methods - they're part of the public interface for future use
#![allow(dead_code)]

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
    add_subview, animate_fade, button_set_action, button_style, color_white, create_button,
    set_text, window_close, window_set_alpha, window_show,
};
use objc::declare::ClassDecl;
use objc::runtime::Sel;
use std::sync::Once;

// Type alias for Objective-C object pointers
type Id = *mut Object;

// Window level constants
const NS_FLOATING_WINDOW_LEVEL: i64 = 3;

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
    transfer_button: Option<usize>,
    action_handler: Option<usize>,
    accumulated_text: String,
}

lazy_static::lazy_static! {
    static ref OVERLAY_STATE: Mutex<TranscriptionOverlayState> = Mutex::new(TranscriptionOverlayState {
        window: None,
        text_field: None,
        status_field: None,
        blur_view: None,
        transfer_button: None,
        action_handler: None,
        accumulated_text: String::new(),
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
            sel!(onTransferToChat:),
            on_transfer_to_chat as extern "C" fn(&Object, Sel, Id),
        );

        ACTION_HANDLER_CLASS = decl.register();
    });
    unsafe { ACTION_HANDLER_CLASS }
}

/// Handler: Transfer transcription text to voice chat overlay
extern "C" fn on_transfer_to_chat(_this: &Object, _cmd: Sel, _sender: Id) {
    let text = get_transcription_text();
    if text.is_empty() {
        return;
    }

    // Transfer text to voice chat draft
    crate::set_voice_chat_draft_text(&text);

    // Show the voice chat overlay
    crate::show_voice_chat_overlay();

    // Hide transcription overlay (user made their choice)
    hide_transcription_overlay();

    info!("Transcription transferred to chat: {} chars", text.len());
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
            window_show(window_ptr as Id);
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

        // Defensive checks for Cocoa classes
        if ns_window_class.is_none()
            || ns_text_field_class.is_none()
            || ns_screen_class.is_none()
            || ns_string_class.is_none()
            || ns_visual_effect_view_class.is_none()
            || ns_color_class.is_none()
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
        let window_width = 420.0;
        let window_height = 180.0;
        let margin = 20.0;
        let corner_radius = 16.0;

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
        let collection_behavior = NSWindowCollectionBehavior::CanJoinAllSpaces;
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

        // === Status indicator (top) ===
        let status_height = 28.0;
        let padding = 16.0;
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
        let initial_status: Id =
            msg_send![ns_string, stringWithUTF8String: c"🔴 Recording...".as_ptr()];
        let _: () = msg_send![status_field, setStringValue: initial_status];

        add_subview(content_view, status_field);

        // === Text field for transcription (main area) ===
        let button_height = 28.0;
        let button_margin = 8.0;
        let text_frame = CGRect {
            origin: CGPoint {
                x: padding,
                y: padding + button_height + button_margin,
            },
            size: CGSize {
                width: window_width - padding * 2.0,
                height: window_height
                    - status_height
                    - padding * 3.0
                    - button_height
                    - button_margin,
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

        // === "Do chatu" button (bottom right) ===
        let button_width = 90.0;
        let button_frame = CGRect {
            origin: CGPoint {
                x: window_width - padding - button_width,
                y: padding,
            },
            size: CGSize {
                width: button_width,
                height: button_height,
            },
        };

        let transfer_button = create_button(button_frame, "Do chatu", button_style::ROUNDED);

        // Create action handler instance
        let handler_class = action_handler_class();
        let action_handler: Id = msg_send![handler_class, alloc];
        let action_handler: Id = msg_send![action_handler, init];

        // Wire up button action
        button_set_action(transfer_button, action_handler, sel!(onTransferToChat:));

        add_subview(content_view, transfer_button);

        // Show the window with fade-in animation
        window_set_alpha(window, 0.0);
        window_show(window);
        animate_fade(window, 1.0, 0.2);

        state.window = Some(window as usize);
        state.text_field = Some(text_field as usize);
        state.status_field = Some(status_field as usize);
        state.blur_view = Some(blur_view as usize);
        state.transfer_button = Some(transfer_button as usize);
        state.action_handler = Some(action_handler as usize);

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
        set_text(status_field_ptr as Id, status);
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
    state.accumulated_text.push_str(delta);

    if let Some(text_field_ptr) = state.text_field {
        set_text(text_field_ptr as Id, &state.accumulated_text);
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

    if let Some(text_field_ptr) = state.text_field {
        set_text(text_field_ptr as Id, text);
    }
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
        set_text(text_field_ptr as Id, "");
    }
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

        // Only hide if this timer is still valid (not superseded)
        if AUTO_HIDE_GENERATION.load(Ordering::SeqCst) == generation
            && AUTO_HIDE_PENDING.load(Ordering::SeqCst)
        {
            hide_transcription_overlay();
            debug!(
                "Transcription overlay auto-hidden after {}s",
                AUTO_HIDE_DELAY_SECS
            );
        }
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
    window_close(window_ptr as Id);
}

fn hide_transcription_overlay_impl() {
    let mut state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(window_ptr) = state.window.take() {
        let window = window_ptr as Id;

        // Fade out animation (0.15s)
        animate_fade(window, 0.0, 0.15);

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
    state.transfer_button = None;
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
}
