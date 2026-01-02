//! macOS UI utilities for hold badge indicator and caret tracing
//!
//! This module provides native macOS functionality for:
//! - Displaying a floating red badge indicator during recording
//! - Tracking text caret position via Accessibility API
//! - Falling back to cursor position when caret is unavailable

// Allow Apple-style constant naming (kAX* prefixes) for Accessibility API
#![allow(non_upper_case_globals)]

use core_foundation::base::TCFType;
use core_foundation::string::CFString;
use core_graphics::geometry::{CGPoint, CGRect, CGSize};
use dispatch::Queue;
use objc::runtime::{Class, Object};
use objc::{msg_send, sel, sel_impl};
use objc2_app_kit::{
    NSBackingStoreType, NSColor, NSEvent, NSWindowCollectionBehavior, NSWindowStyleMask,
};
use std::ptr;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;
use tracing::{debug, warn};

// Type alias for Objective-C object pointers (compatible with objc crate msg_send!)
type Id = *mut Object;

// Accessibility API bindings (use raw pointers compatible with C FFI)
type AXId = *mut std::ffi::c_void;

#[link(name = "ApplicationServices", kind = "framework")]
extern "C" {
    fn AXUIElementCopyAttributeValue(element: AXId, attribute: AXId, value: *mut AXId) -> i32;
    fn AXUIElementCreateSystemWide() -> AXId;
    fn AXValueGetValue(value: AXId, type_: i32, value_ptr: *mut std::ffi::c_void) -> bool;
    fn CFRelease(cf: AXId);
}

// AX constants
const kAXErrorSuccess: i32 = 0;
const kAXFocusedUIElementAttribute: &str = "AXFocusedUIElement";
const kAXRoleAttribute: &str = "AXRole";
const kAXSelectedTextRangeAttribute: &str = "AXSelectedTextRange";
const kAXPositionAttribute: &str = "AXPosition";
const kAXSizeAttribute: &str = "AXSize";

// AXValue types
const kAXValueCGPointType: i32 = 1;
const kAXValueCGSizeType: i32 = 2;
#[allow(dead_code)]
const kAXValueCFRangeType: i32 = 3;

// Window level constants
const NS_STATUS_WINDOW_LEVEL: i64 = 25;

/// Configuration for the hold badge
#[derive(Debug, Clone)]
pub struct HoldBadgeConfig {
    /// Diameter of the badge circle in pixels
    pub diameter: f64,
    /// Offset from caret/cursor position (x, y)
    pub offset: (f64, f64),
    /// Update interval in milliseconds
    pub update_interval_ms: u64,
    /// Badge color (R, G, B, A)
    pub color: (f64, f64, f64, f64),
}

impl Default for HoldBadgeConfig {
    fn default() -> Self {
        Self {
            diameter: 12.0,
            offset: (10.0, -10.0),
            update_interval_ms: 150,
            color: (1.0, 0.0, 0.0, 0.8), // Red with 80% opacity
        }
    }
}

/// Hold badge state
struct HoldBadgeState {
    window: Option<usize>, // Store as usize to make it Send
    timer_running: bool,
    config: HoldBadgeConfig,
}

lazy_static::lazy_static! {
    static ref BADGE_STATE: Arc<Mutex<HoldBadgeState>> = Arc::new(Mutex::new(HoldBadgeState {
        window: None,
        timer_running: false,
        config: HoldBadgeConfig::default(),
    }));
}

/// Check if the currently focused element accepts text input
pub fn focused_element_accepts_text() -> bool {
    unsafe {
        let system_wide = AXUIElementCreateSystemWide();
        if system_wide.is_null() {
            return false;
        }

        let mut focused_element: AXId = ptr::null_mut();
        let attr_name = CFString::new(kAXFocusedUIElementAttribute);
        let result = AXUIElementCopyAttributeValue(
            system_wide,
            attr_name.as_concrete_TypeRef() as AXId,
            &mut focused_element,
        );

        CFRelease(system_wide);

        if result != kAXErrorSuccess || focused_element.is_null() {
            return false;
        }

        // Get role attribute
        let mut role_value: AXId = ptr::null_mut();
        let role_attr = CFString::new(kAXRoleAttribute);
        let role_result = AXUIElementCopyAttributeValue(
            focused_element,
            role_attr.as_concrete_TypeRef() as AXId,
            &mut role_value,
        );

        CFRelease(focused_element);

        if role_result != kAXErrorSuccess || role_value.is_null() {
            return false;
        }

        // Convert role to string
        let role_str = CFString::wrap_under_get_rule(role_value as *const _);
        let role = role_str.to_string();
        CFRelease(role_value);

        // Check if role indicates text input
        matches!(
            role.as_str(),
            "AXTextArea" | "AXTextField" | "AXComboBox" | "AXTextView" | "AXWebArea"
        )
    }
}

/// Get the current text caret position in screen coordinates
pub fn get_caret_position() -> Option<(f64, f64)> {
    unsafe {
        let system_wide = AXUIElementCreateSystemWide();
        if system_wide.is_null() {
            return None;
        }

        let mut focused_element: AXId = ptr::null_mut();
        let attr_name = CFString::new(kAXFocusedUIElementAttribute);
        let result = AXUIElementCopyAttributeValue(
            system_wide,
            attr_name.as_concrete_TypeRef() as AXId,
            &mut focused_element,
        );

        CFRelease(system_wide);

        if result != kAXErrorSuccess || focused_element.is_null() {
            return None;
        }

        // Get selected text range
        let mut range_value: AXId = ptr::null_mut();
        let range_attr = CFString::new(kAXSelectedTextRangeAttribute);
        let range_result = AXUIElementCopyAttributeValue(
            focused_element,
            range_attr.as_concrete_TypeRef() as AXId,
            &mut range_value,
        );

        if range_result != kAXErrorSuccess || range_value.is_null() {
            CFRelease(focused_element);
            return None;
        }

        // Extract range
        #[repr(C)]
        struct CFRange {
            location: i64,
            length: i64,
        }

        let mut cf_range = CFRange {
            location: 0,
            length: 0,
        };

        let range_ok = AXValueGetValue(
            range_value,
            kAXValueCFRangeType,
            &mut cf_range as *mut _ as *mut std::ffi::c_void,
        );

        CFRelease(range_value);

        if !range_ok {
            CFRelease(focused_element);
            return None;
        }

        // Try to get position and size of the focused element
        let mut position_value: AXId = ptr::null_mut();
        let position_attr = CFString::new(kAXPositionAttribute);
        let position_result = AXUIElementCopyAttributeValue(
            focused_element,
            position_attr.as_concrete_TypeRef() as AXId,
            &mut position_value,
        );

        let mut size_value: AXId = ptr::null_mut();
        let size_attr = CFString::new(kAXSizeAttribute);
        let size_result = AXUIElementCopyAttributeValue(
            focused_element,
            size_attr.as_concrete_TypeRef() as AXId,
            &mut size_value,
        );

        CFRelease(focused_element);

        if position_result != kAXErrorSuccess
            || position_value.is_null()
            || size_result != kAXErrorSuccess
            || size_value.is_null()
        {
            if !position_value.is_null() {
                CFRelease(position_value);
            }
            if !size_value.is_null() {
                CFRelease(size_value);
            }
            return None;
        }

        // Extract position
        let mut position = CGPoint { x: 0.0, y: 0.0 };
        let position_ok = AXValueGetValue(
            position_value,
            kAXValueCGPointType,
            &mut position as *mut _ as *mut std::ffi::c_void,
        );

        CFRelease(position_value);

        // Extract size
        let mut size = CGSize {
            width: 0.0,
            height: 0.0,
        };
        let size_ok = AXValueGetValue(
            size_value,
            kAXValueCGSizeType,
            &mut size as *mut _ as *mut std::ffi::c_void,
        );

        CFRelease(size_value);

        if !position_ok || !size_ok {
            return None;
        }

        // Estimate caret position (top-left of element + small offset)
        // For better accuracy, we'd need to parse the text layout, but this is a reasonable approximation
        Some((position.x, position.y + size.height / 2.0))
    }
}

/// Get the current mouse cursor position in screen coordinates
pub fn get_cursor_position() -> (f64, f64) {
    let mouse_location = NSEvent::mouseLocation();
    (mouse_location.x, mouse_location.y)
}

/// Get the best available position for the badge (caret or cursor)
fn get_badge_position() -> (f64, f64) {
    get_caret_position().unwrap_or_else(get_cursor_position)
}

/// Create the hold badge window
unsafe fn create_badge_window(config: &HoldBadgeConfig) -> Id {
    let ns_window = Class::get("NSWindow").unwrap();

    // Get initial position
    let (x, y) = get_badge_position();
    let adjusted_x = x + config.offset.0;
    let adjusted_y = y + config.offset.1;
    debug!(
        "Badge position: raw=({:.1}, {:.1}), adjusted=({:.1}, {:.1}), diameter={}",
        x, y, adjusted_x, adjusted_y, config.diameter
    );

    // Create window frame using CGRect (screen coordinates)
    let window_frame = CGRect {
        origin: CGPoint {
            x: adjusted_x,
            y: adjusted_y,
        },
        size: CGSize {
            width: config.diameter,
            height: config.diameter,
        },
    };

    // Create window
    let window: Id = msg_send![ns_window, alloc];
    let style_mask = NSWindowStyleMask::Borderless;
    let backing = NSBackingStoreType::Buffered;
    let window: Id = msg_send![
        window,
        initWithContentRect: window_frame
        styleMask: style_mask
        backing: backing
        defer: false
    ];

    // Configure window for floating transparent overlay
    let clear_color = NSColor::clearColor();
    let clear_color_ptr = &*clear_color as *const _ as Id;
    let _: () = msg_send![window, setOpaque: false];
    let _: () = msg_send![window, setBackgroundColor: clear_color_ptr];
    let _: () = msg_send![window, setIgnoresMouseEvents: true];
    let _: () = msg_send![window, setLevel: NS_STATUS_WINDOW_LEVEL];
    let collection_behavior = NSWindowCollectionBehavior::CanJoinAllSpaces;
    let _: () = msg_send![window, setCollectionBehavior: collection_behavior];

    // Enable layer-backed views for better transparency/compositing
    let content_view: Id = msg_send![window, contentView];
    let _: () = msg_send![content_view, setWantsLayer: true];

    // Create badge view (circular red indicator)
    let badge_view = create_badge_view(config);
    let _: () = msg_send![content_view, addSubview: badge_view];

    // Force the view to display
    let _: () = msg_send![badge_view, setNeedsDisplay: true];

    window
}

/// Create the circular badge view using CALayer for reliable rendering
unsafe fn create_badge_view(config: &HoldBadgeConfig) -> Id {
    // Use a plain NSView with a CALayer for drawing
    let ns_view = Class::get("NSView").unwrap();
    let view: Id = msg_send![ns_view, alloc];
    let frame = CGRect {
        origin: CGPoint { x: 0.0, y: 0.0 },
        size: CGSize {
            width: config.diameter,
            height: config.diameter,
        },
    };
    let view: Id = msg_send![view, initWithFrame: frame];

    // Enable layer-backing
    let _: () = msg_send![view, setWantsLayer: true];

    // Get the layer
    let layer: Id = msg_send![view, layer];
    if layer.is_null() {
        warn!("Badge layer is null - badge will not be visible");
        return view;
    }

    // Configure the layer to draw a circle
    // Set background color from config (default: red with 80% opacity)
    let cg_color = create_cg_color(
        config.color.0,
        config.color.1,
        config.color.2,
        config.color.3,
    );
    let _: () = msg_send![layer, setBackgroundColor: cg_color];
    CGColorRelease(cg_color);

    // Make it circular by setting corner radius to half the diameter
    let corner_radius = config.diameter / 2.0;
    let _: () = msg_send![layer, setCornerRadius: corner_radius];

    // Ensure the layer clips to bounds (for the circle shape)
    let _: () = msg_send![layer, setMasksToBounds: true];

    view
}

// CGColor functions
#[link(name = "CoreGraphics", kind = "framework")]
extern "C" {
    fn CGColorCreate(
        space: *const std::ffi::c_void,
        components: *const f64,
    ) -> *const std::ffi::c_void;
    fn CGColorSpaceCreateDeviceRGB() -> *const std::ffi::c_void;
    fn CGColorSpaceRelease(space: *const std::ffi::c_void);
    fn CGColorRelease(color: *const std::ffi::c_void);
}

/// Create a CGColor from RGBA components
unsafe fn create_cg_color(r: f64, g: f64, b: f64, a: f64) -> *const std::ffi::c_void {
    let color_space = CGColorSpaceCreateDeviceRGB();
    let components: [f64; 4] = [r, g, b, a];
    let color = CGColorCreate(color_space, components.as_ptr());
    CGColorSpaceRelease(color_space);
    color
}

/// Update the badge window position
unsafe fn update_badge_position(window: Id, config: &HoldBadgeConfig) {
    let (x, y) = get_badge_position();
    let adjusted_x = x + config.offset.0;
    let adjusted_y = y + config.offset.1;

    let new_origin = CGPoint {
        x: adjusted_x,
        y: adjusted_y,
    };
    let _: () = msg_send![window, setFrameOrigin: new_origin];
}

/// Show the hold badge and start position tracking
pub fn show_hold_badge() {
    show_hold_badge_with_config(HoldBadgeConfig::default());
}

/// Internal implementation that must run on the main thread
fn show_hold_badge_impl(config: HoldBadgeConfig) {
    debug!("Showing hold badge (diameter={})", config.diameter);
    unsafe {
        let mut state = BADGE_STATE.lock().unwrap();

        // Hide existing badge if any
        if let Some(window_ptr) = state.window {
            let window = window_ptr as Id;
            let _: () = msg_send![window, close];
            state.window = None;
        }

        // Create new badge window (MUST be on main thread)
        let window = create_badge_window(&config);

        // Make window visible - use orderFrontRegardless which works even when app is not active
        let _: () = msg_send![window, orderFrontRegardless];

        // Force content view to redraw
        let content_view: Id = msg_send![window, contentView];
        let _: () = msg_send![content_view, setNeedsDisplay: true];

        state.window = Some(window as usize);
        state.config = config.clone();
        state.timer_running = true;

        // Start position update timer
        let update_interval = config.update_interval_ms;

        thread::spawn(move || {
            while BADGE_STATE.lock().unwrap().timer_running {
                thread::sleep(Duration::from_millis(update_interval));

                let state = BADGE_STATE.lock().unwrap();
                if !state.timer_running {
                    break;
                }

                if let Some(window_ptr) = state.window {
                    // Position updates also need main thread
                    Queue::main().exec_async(move || {
                        let window = window_ptr as Id;
                        let state = BADGE_STATE.lock().unwrap();
                        update_badge_position(window, &state.config);
                    });
                }
            }
        });
    }
}

/// Show the hold badge with custom configuration
/// This dispatches to the main thread for thread safety with NSWindow
pub fn show_hold_badge_with_config(config: HoldBadgeConfig) {
    // Check if we're already on the main thread by checking thread name
    // Note: exec_sync on main queue from main thread causes deadlock
    let is_main_thread = std::thread::current().name() == Some("main");

    if is_main_thread {
        show_hold_badge_impl(config);
    } else {
        // Dispatch to main thread - NSWindow MUST be created on main thread
        // Using exec_async to avoid deadlock when called from tokio runtime
        Queue::main().exec_async(move || {
            show_hold_badge_impl(config);
        });
    }
}

/// Hide the hold badge and stop position tracking
/// This dispatches to the main thread for thread safety with NSWindow
pub fn hide_hold_badge() {
    debug!("Hiding hold badge");

    // Stop the timer first (can be done on any thread)
    {
        let mut state = BADGE_STATE.lock().unwrap();
        state.timer_running = false;
    }

    // Dispatch window close to main thread
    Queue::main().exec_async(|| unsafe {
        let mut state = BADGE_STATE.lock().unwrap();
        if let Some(window_ptr) = state.window {
            let window = window_ptr as Id;
            let _: () = msg_send![window, close];
            state.window = None;
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cursor_position() {
        let (x, y) = get_cursor_position();
        // Just verify we get some coordinates
        assert!(x >= 0.0);
        assert!(y >= 0.0);
    }

    #[test]
    fn test_focused_element_check() {
        // This will return false in test environment (no GUI)
        // but verifies the function doesn't crash
        let _ = focused_element_accepts_text();
    }

    #[test]
    fn test_badge_config_default() {
        let config = HoldBadgeConfig::default();
        assert_eq!(config.diameter, 12.0);
        assert_eq!(config.offset, (10.0, -10.0));
        assert_eq!(config.update_interval_ms, 150);
    }
}
