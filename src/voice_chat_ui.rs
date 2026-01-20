//! Voice Chat UI overlay for displaying streaming responses.
//!
//! This module provides a floating overlay window that:
//! - Shows status during voice chat (Recording, Thinking, etc.)
//! - Displays streaming LLM response text
//! - Auto-hides after completion

// Allow unexpected cfgs from objc crate's msg_send! macro
#![allow(unexpected_cfgs)]
// Allow unused API methods - they're part of the public interface for future use
#![allow(dead_code)]

use codescribe_core::config::{Config, OverlayPositionMode};
use core_graphics::geometry::{CGPoint, CGRect, CGSize};
use dispatch::Queue;
use objc::runtime::{Class, Object};
use objc::{msg_send, sel, sel_impl};
use objc2_app_kit::{NSBackingStoreType, NSColor, NSWindowCollectionBehavior, NSWindowStyleMask};
use std::sync::Mutex;
use tracing::{debug, info};

// Type alias for Objective-C object pointers
type Id = *mut Object;

// Window level constants
const NS_FLOATING_WINDOW_LEVEL: i64 = 3;

/// Configuration for the voice chat overlay
#[derive(Debug, Clone)]
pub struct VoiceChatOverlayConfig {
    /// Width of the overlay window in pixels
    pub width: f64,
    /// Height of the overlay window in pixels
    pub height: f64,
    /// Auto-hide timeout in seconds (0 = no auto-hide)
    pub auto_hide_timeout_secs: u64,
}

impl Default for VoiceChatOverlayConfig {
    fn default() -> Self {
        Self {
            width: 400.0,
            height: 200.0,
            auto_hide_timeout_secs: 5,
        }
    }
}

/// Voice chat overlay state
struct VoiceChatOverlayState {
    window: Option<usize>,
    text_view: Option<usize>,
    status_field: Option<usize>,
    accumulated_text: String,
}

lazy_static::lazy_static! {
    static ref OVERLAY_STATE: Mutex<VoiceChatOverlayState> = Mutex::new(VoiceChatOverlayState {
        window: None,
        text_view: None,
        status_field: None,
        accumulated_text: String::new(),
    });
}

/// Show the voice chat overlay window
pub fn show_voice_chat_overlay() {
    Queue::main().exec_async(|| {
        show_voice_chat_overlay_impl();
    });
}

/// Show the voice chat overlay with custom configuration
pub fn show_voice_chat_overlay_with_config(_config: VoiceChatOverlayConfig) {
    // Currently uses default dimensions, config reserved for future use
    Queue::main().exec_async(|| {
        show_voice_chat_overlay_impl();
    });
}

fn show_voice_chat_overlay_impl() {
    unsafe {
        let mut state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());

        // Close existing window if any
        if let Some(window_ptr) = state.window.take() {
            let window = window_ptr as Id;
            let _: () = msg_send![window, close];
        }

        state.accumulated_text.clear();

        let ns_window = Class::get("NSWindow").unwrap();
        let ns_text_field = Class::get("NSTextField").unwrap();

        // Get screen size to position the overlay
        let ns_screen = Class::get("NSScreen").unwrap();
        let main_screen: Id = msg_send![ns_screen, mainScreen];
        let visible_frame: CGRect = msg_send![main_screen, visibleFrame];

        // Load config for position logic
        let config = Config::load();

        // Create window dimensions
        let window_width = 400.0;
        let window_height = 200.0;
        let margin = 16.0;

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

        // Create window with rounded corners style (Title + FullSizeContent + Hidden Title)
        let window: Id = msg_send![ns_window, alloc];
        let style_mask = NSWindowStyleMask::Titled | NSWindowStyleMask::FullSizeContentView;
        let backing = NSBackingStoreType::Buffered;
        let window: Id = msg_send![
            window,
            initWithContentRect: frame
            styleMask: style_mask
            backing: backing
            defer: false
        ];

        // Configure rounded corners and dragging
        let _: () = msg_send![window, setTitleVisibility: 1]; // NSWindowTitleHidden
        let _: () = msg_send![window, setTitlebarAppearsTransparent: true];
        let _: () = msg_send![window, setMovableByWindowBackground: true];

        // Configure window appearance
        let bg_color = NSColor::colorWithCalibratedRed_green_blue_alpha(0.1, 0.1, 0.1, 0.95);
        let bg_color_ptr = &*bg_color as *const _ as Id;
        let _: () = msg_send![window, setOpaque: false];
        let _: () = msg_send![window, setBackgroundColor: bg_color_ptr];
        let _: () = msg_send![window, setLevel: NS_FLOATING_WINDOW_LEVEL];
        let collection_behavior = NSWindowCollectionBehavior::CanJoinAllSpaces;
        let _: () = msg_send![window, setCollectionBehavior: collection_behavior];

        // Get content view
        let content_view: Id = msg_send![window, contentView];

        // Create status header bar at top
        let header_height = 30.0;
        let status_frame = CGRect {
            origin: CGPoint {
                x: 0.0,
                y: window_height - header_height,
            },
            size: CGSize {
                width: window_width,
                height: header_height,
            },
        };
        let status_field: Id = msg_send![ns_text_field, alloc];
        let status_field: Id = msg_send![status_field, initWithFrame: status_frame];
        let _: () = msg_send![status_field, setBezeled: false];
        let _: () = msg_send![status_field, setDrawsBackground: true];
        let _: () = msg_send![status_field, setEditable: false];
        let _: () = msg_send![status_field, setSelectable: false];

        // Header background color
        let header_color = NSColor::colorWithCalibratedRed_green_blue_alpha(0.2, 0.2, 0.2, 0.8);
        let header_color_ptr = &*header_color as *const _ as Id;
        let _: () = msg_send![status_field, setBackgroundColor: header_color_ptr];

        // Alignment center (1 = Center in older constants, or 2 depending on version, safely assumed 1 for now)
        // Actually let's keep left aligned but with padding if possible.
        // For now standard left is fine, just added background.

        let white_color = NSColor::whiteColor();
        let white_color_ptr = &*white_color as *const _ as Id;
        let _: () = msg_send![status_field, setTextColor: white_color_ptr];

        let ns_string = Class::get("NSString").unwrap();
        let initial_status: Id =
            msg_send![ns_string, stringWithUTF8String: c"Recording...".as_ptr()];
        let _: () = msg_send![status_field, setStringValue: initial_status];

        let _: () = msg_send![content_view, addSubview: status_field];

        // Create scrollable text view for response
        let scroll_frame = CGRect {
            origin: CGPoint { x: 10.0, y: 10.0 },
            size: CGSize {
                width: window_width - 20.0,
                height: window_height - 50.0,
            },
        };

        let ns_scroll_view = Class::get("NSScrollView").unwrap();
        let scroll_view: Id = msg_send![ns_scroll_view, alloc];
        let scroll_view: Id = msg_send![scroll_view, initWithFrame: scroll_frame];
        let _: () = msg_send![scroll_view, setHasVerticalScroller: true];
        let _: () = msg_send![scroll_view, setBorderType: 0]; // NSNoBorder
        let _: () = msg_send![scroll_view, setDrawsBackground: false];

        // Content rect for text view
        let content_size: CGSize = msg_send![scroll_view, contentSize];
        let text_frame = CGRect {
            origin: CGPoint { x: 0.0, y: 0.0 },
            size: content_size,
        };

        let ns_text_view = Class::get("NSTextView").unwrap();
        let text_view: Id = msg_send![ns_text_view, alloc];
        let text_view: Id = msg_send![text_view, initWithFrame: text_frame];

        let _: () =
            msg_send![text_view, setMinSize: CGSize { width: 0.0, height: content_size.height }];
        let _: () = msg_send![text_view, setMaxSize: CGSize { width: f64::MAX, height: f64::MAX }];
        let _: () = msg_send![text_view, setVerticallyResizable: true];
        let _: () = msg_send![text_view, setHorizontallyResizable: false];
        let _: () = msg_send![text_view, setAutoresizingMask: 2]; // NSViewWidthSizable

        let text_container: Id = msg_send![text_view, textContainer];
        let _: () = msg_send![text_container, setWidthTracksTextView: true];

        let _: () = msg_send![text_view, setEditable: false];
        let _: () = msg_send![text_view, setSelectable: true];
        let _: () = msg_send![text_view, setTextColor: white_color_ptr];
        let _: () = msg_send![text_view, setDrawsBackground: false];

        let _: () = msg_send![scroll_view, setDocumentView: text_view];
        let _: () = msg_send![content_view, addSubview: scroll_view];

        // Show the window
        let _: () = msg_send![window, orderFrontRegardless];

        state.window = Some(window as usize);
        state.text_view = Some(text_view as usize);
        state.status_field = Some(status_field as usize);

        info!("Voice chat overlay shown");
    }
}

/// Update the status text in the overlay
pub fn update_voice_chat_status(status: &str) {
    let status_owned = status.to_string();
    Queue::main().exec_async(move || {
        update_voice_chat_status_impl(&status_owned);
    });
}

fn update_voice_chat_status_impl(status: &str) {
    unsafe {
        let state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(status_field_ptr) = state.status_field {
            let status_field = status_field_ptr as Id;
            let ns_string = Class::get("NSString").unwrap();

            // Create null-terminated C string
            let mut c_str = status.as_bytes().to_vec();
            c_str.push(0);

            let ns_str: Id = msg_send![ns_string, stringWithUTF8String: c_str.as_ptr()];
            let _: () = msg_send![status_field, setStringValue: ns_str];
        }
    }
}

/// Append a delta (streaming token) to the overlay text
pub fn append_voice_chat_delta(delta: &str) {
    let delta_owned = delta.to_string();
    Queue::main().exec_async(move || {
        append_voice_chat_delta_impl(&delta_owned);
    });
}

#[repr(C)]
#[derive(Debug, Copy, Clone)]
struct NSRange {
    location: usize,
    length: usize,
}

fn append_voice_chat_delta_impl(delta: &str) {
    unsafe {
        let mut state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
        state.accumulated_text.push_str(delta);

        if let Some(text_view_ptr) = state.text_view {
            let text_view = text_view_ptr as Id;
            let ns_string = Class::get("NSString").unwrap();

            // Create null-terminated C string
            let mut c_str = state.accumulated_text.as_bytes().to_vec();
            c_str.push(0);

            let ns_str: Id = msg_send![ns_string, stringWithUTF8String: c_str.as_ptr()];
            let _: () = msg_send![text_view, setString: ns_str];

            // Auto-scroll to end
            let length: usize = msg_send![ns_str, length];
            let range = NSRange {
                location: length,
                length: 0,
            };
            let _: () = msg_send![text_view, scrollRangeToVisible: range];
        }
    }
}

/// Set the full text in the overlay
pub fn set_voice_chat_text(text: &str) {
    let text_owned = text.to_string();
    Queue::main().exec_async(move || {
        set_voice_chat_text_impl(&text_owned);
    });
}

fn set_voice_chat_text_impl(text: &str) {
    unsafe {
        let mut state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
        state.accumulated_text = text.to_string();

        if let Some(text_view_ptr) = state.text_view {
            let text_view = text_view_ptr as Id;
            let ns_string = Class::get("NSString").unwrap();

            // Create null-terminated C string
            let mut c_str = text.as_bytes().to_vec();
            c_str.push(0);

            let ns_str: Id = msg_send![ns_string, stringWithUTF8String: c_str.as_ptr()];
            let _: () = msg_send![text_view, setString: ns_str];

            // Auto-scroll to end
            let length: usize = msg_send![ns_str, length];
            let range = NSRange {
                location: length,
                length: 0,
            };
            let _: () = msg_send![text_view, scrollRangeToVisible: range];
        }
    }
}

/// Get the accumulated text from the overlay
pub fn get_accumulated_text() -> String {
    let state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
    state.accumulated_text.clone()
}

/// Clear the text content of the overlay
pub fn clear_voice_chat_text() {
    Queue::main().exec_async(|| {
        clear_voice_chat_text_impl();
    });
}

fn clear_voice_chat_text_impl() {
    unsafe {
        let mut state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
        state.accumulated_text.clear();

        if let Some(text_view_ptr) = state.text_view {
            let text_view = text_view_ptr as Id;
            let ns_string = Class::get("NSString").unwrap();
            let empty: Id = msg_send![ns_string, stringWithUTF8String: c"".as_ptr()];
            let _: () = msg_send![text_view, setString: empty];
        }
    }
}

/// Check if the voice chat overlay is currently visible
pub fn is_voice_chat_overlay_visible() -> bool {
    let state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
    state.window.is_some()
}

/// Reset the auto-hide timer (placeholder for future implementation)
pub fn reset_voice_chat_activity() {
    // Currently no auto-hide timer, but function exists for API compatibility
    debug!("reset_voice_chat_activity called");
}

/// Hide the voice chat overlay window
pub fn hide_voice_chat_overlay() {
    Queue::main().exec_async(|| {
        hide_voice_chat_overlay_impl();
    });
}

fn hide_voice_chat_overlay_impl() {
    unsafe {
        let mut state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(window_ptr) = state.window.take() {
            let window = window_ptr as Id;
            let _: () = msg_send![window, close];
            debug!("Voice chat overlay hidden");
        }
        state.text_view = None;
        state.status_field = None;
        state.accumulated_text.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_accumulated_text() {
        // Just verify the function doesn't panic
        let _ = get_accumulated_text();
    }

    #[test]
    fn test_overlay_config_default() {
        let config = VoiceChatOverlayConfig::default();
        assert_eq!(config.width, 400.0);
        assert_eq!(config.height, 200.0);
        assert_eq!(config.auto_hide_timeout_secs, 5);
    }

    #[test]
    fn test_overlay_config_custom() {
        let config = VoiceChatOverlayConfig {
            width: 600.0,
            height: 300.0,
            auto_hide_timeout_secs: 10,
        };
        assert_eq!(config.width, 600.0);
        assert_eq!(config.height, 300.0);
        assert_eq!(config.auto_hide_timeout_secs, 10);
    }

    #[test]
    fn test_overlay_config_clone() {
        let config = VoiceChatOverlayConfig::default();
        let cloned = config.clone();
        assert_eq!(cloned.width, config.width);
        assert_eq!(cloned.height, config.height);
    }

    #[test]
    fn test_overlay_config_debug() {
        let config = VoiceChatOverlayConfig::default();
        let debug_str = format!("{:?}", config);
        assert!(debug_str.contains("VoiceChatOverlayConfig"));
        assert!(debug_str.contains("400"));
    }

    #[test]
    fn test_overlay_state_initial() {
        // Verify the initial state is empty
        let state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
        // Window should be None initially (unless another test created it)
        // Just verify we can access the state without panic
        let _ = state.accumulated_text.len();
    }

    #[test]
    fn test_is_overlay_visible_returns_bool() {
        // Just verify the function returns a bool without panic
        let visible = is_voice_chat_overlay_visible();
        // Can be either true or false depending on test order
        let _ = visible;
    }
}
