//! Native AppKit UI helpers for CodeScribe
//!
//! Reduces msg_send! boilerplate by providing high-level functions for common UI patterns.
//! These helpers wrap Objective-C calls in safe, reusable Rust functions.
//!
//! # Safety
//! All functions in this module operate on raw Objective-C pointers (`Id = *mut Object`).
//! Callers must ensure pointers are valid. This is standard for Rust-ObjC FFI.
//!
//! Created by M&K (c)2026 VetCoders

#![allow(unexpected_cfgs)]
#![allow(clippy::not_unsafe_ptr_arg_deref)]

use core_graphics::geometry::{CGPoint, CGRect, CGSize};
use objc::runtime::{Class, Object};
use objc::{msg_send, sel, sel_impl};
use objc2_app_kit::{NSBackingStoreType, NSWindowCollectionBehavior, NSWindowStyleMask};
use std::ffi::CString;

/// Type alias for Objective-C object pointers
pub type Id = *mut Object;

/// Window level constants
pub const NS_FLOATING_WINDOW_LEVEL: i64 = 3;
pub const NS_STATUS_WINDOW_LEVEL: i64 = 25;

// ============================================================================
// Color Helpers
// ============================================================================

/// Create an NSColor from RGBA values (0.0-1.0)
pub fn color_rgba(r: f64, g: f64, b: f64, a: f64) -> Id {
    unsafe {
        let ns_color = Class::get("NSColor").unwrap();
        msg_send![ns_color, colorWithRed: r green: g blue: b alpha: a]
    }
}

/// Create white color with optional alpha
pub fn color_white(alpha: f64) -> Id {
    unsafe {
        let ns_color = Class::get("NSColor").unwrap();
        msg_send![ns_color, colorWithWhite: 1.0f64 alpha: alpha]
    }
}

/// Create clear (transparent) color
pub fn color_clear() -> Id {
    unsafe {
        let ns_color = Class::get("NSColor").unwrap();
        msg_send![ns_color, clearColor]
    }
}

// ============================================================================
// String Helpers
// ============================================================================

/// Create an NSString from a Rust &str
pub fn ns_string(s: &str) -> Id {
    unsafe {
        let ns_string = Class::get("NSString").unwrap();
        let c_str = CString::new(s).unwrap_or_else(|_| CString::new("").unwrap());
        msg_send![ns_string, stringWithUTF8String: c_str.as_ptr()]
    }
}

// ============================================================================
// Label / TextField Helpers
// ============================================================================

/// Configuration for creating a label
pub struct LabelConfig {
    pub frame: CGRect,
    pub text: String,
    pub font_size: f64,
    pub bold: bool,
    pub text_color: Id,
    pub background_color: Option<Id>,
    pub selectable: bool,
    pub editable: bool,
}

impl Default for LabelConfig {
    fn default() -> Self {
        Self {
            frame: CGRect::new(&CGPoint::new(0.0, 0.0), &CGSize::new(100.0, 24.0)),
            text: String::new(),
            font_size: 13.0,
            bold: false,
            text_color: unsafe {
                let ns_color = Class::get("NSColor").unwrap();
                msg_send![ns_color, whiteColor]
            },
            background_color: None,
            selectable: false,
            editable: false,
        }
    }
}

/// Create a label (non-editable text field)
pub fn create_label(config: LabelConfig) -> Id {
    unsafe {
        let ns_text_field = Class::get("NSTextField").unwrap();
        let ns_font = Class::get("NSFont").unwrap();

        let field: Id = msg_send![ns_text_field, alloc];
        let field: Id = msg_send![field, initWithFrame: config.frame];

        let _: () = msg_send![field, setBezeled: false];
        let _: () = msg_send![field, setEditable: config.editable];
        let _: () = msg_send![field, setSelectable: config.selectable];

        if let Some(bg) = config.background_color {
            let _: () = msg_send![field, setDrawsBackground: true];
            let _: () = msg_send![field, setBackgroundColor: bg];
        } else {
            let _: () = msg_send![field, setDrawsBackground: false];
        }

        let _: () = msg_send![field, setTextColor: config.text_color];

        let font: Id = if config.bold {
            msg_send![ns_font, boldSystemFontOfSize: config.font_size]
        } else {
            msg_send![ns_font, systemFontOfSize: config.font_size]
        };
        let _: () = msg_send![field, setFont: font];

        let text = ns_string(&config.text);
        let _: () = msg_send![field, setStringValue: text];

        field
    }
}

/// Quick label creation with minimal config
pub fn label(frame: CGRect, text: &str) -> Id {
    create_label(LabelConfig {
        frame,
        text: text.to_string(),
        ..Default::default()
    })
}

/// Quick label with custom font size
pub fn label_sized(frame: CGRect, text: &str, font_size: f64, bold: bool) -> Id {
    create_label(LabelConfig {
        frame,
        text: text.to_string(),
        font_size,
        bold,
        ..Default::default()
    })
}

// ============================================================================
// Button Helpers
// ============================================================================

/// Button style constants
pub mod button_style {
    pub const ROUNDED: isize = 1;
    pub const REGULAR_SQUARE: isize = 2;
    pub const SHADOWLESS_SQUARE: isize = 6;
    pub const SMALL_SQUARE: isize = 10;
    pub const INLINE: isize = 15;
}

/// Create a button with title and action
pub fn create_button(frame: CGRect, title: &str, style: isize) -> Id {
    unsafe {
        let ns_button = Class::get("NSButton").unwrap();

        let btn: Id = msg_send![ns_button, alloc];
        let btn: Id = msg_send![btn, initWithFrame: frame];

        let title_str = ns_string(title);
        let _: () = msg_send![btn, setTitle: title_str];
        let _: () = msg_send![btn, setBezelStyle: style];

        btn
    }
}

/// Set button target and action
pub fn button_set_action(button: Id, target: Id, action: objc::runtime::Sel) {
    unsafe {
        let _: () = msg_send![button, setTarget: target];
        let _: () = msg_send![button, setAction: action];
    }
}

/// Quick rounded button
pub fn button(frame: CGRect, title: &str) -> Id {
    create_button(frame, title, button_style::ROUNDED)
}

// ============================================================================
// Checkbox Helpers
// ============================================================================

/// Create a checkbox (switch style button)
pub fn create_checkbox(frame: CGRect, title: &str, checked: bool) -> Id {
    unsafe {
        let ns_button = Class::get("NSButton").unwrap();

        let btn: Id = msg_send![ns_button, alloc];
        let btn: Id = msg_send![btn, initWithFrame: frame];

        let _: () = msg_send![btn, setButtonType: 3_isize]; // NSSwitchButton

        let title_str = ns_string(title);
        let _: () = msg_send![btn, setTitle: title_str];

        let state: isize = if checked { 1 } else { 0 };
        let _: () = msg_send![btn, setState: state];

        btn
    }
}

// ============================================================================
// Scroll View + Text View Helpers
// ============================================================================

/// Create a scroll view with embedded text view for multi-line text display
pub fn create_scrollable_text_view(frame: CGRect, editable: bool) -> (Id, Id) {
    unsafe {
        let ns_scroll_view = Class::get("NSScrollView").unwrap();
        let ns_text_view = Class::get("NSTextView").unwrap();
        let ns_color = Class::get("NSColor").unwrap();

        // Create scroll view
        let scroll: Id = msg_send![ns_scroll_view, alloc];
        let scroll: Id = msg_send![scroll, initWithFrame: frame];
        let _: () = msg_send![scroll, setHasVerticalScroller: true];
        let _: () = msg_send![scroll, setHasHorizontalScroller: false];
        let _: () = msg_send![scroll, setDrawsBackground: false];
        let _: () = msg_send![scroll, setBorderType: 0_isize]; // NSNoBorder

        // Create text view with same size
        let text_frame = CGRect::new(
            &CGPoint::new(0.0, 0.0),
            &CGSize::new(frame.size.width, frame.size.height),
        );
        let text_view: Id = msg_send![ns_text_view, alloc];
        let text_view: Id = msg_send![text_view, initWithFrame: text_frame];

        let _: () = msg_send![text_view, setEditable: editable];
        let _: () = msg_send![text_view, setSelectable: true];

        // Transparent background
        let clear: Id = msg_send![ns_color, clearColor];
        let _: () = msg_send![text_view, setBackgroundColor: clear];

        // White text
        let white: Id = msg_send![ns_color, whiteColor];
        let _: () = msg_send![text_view, setTextColor: white];

        // Auto-resize with scroll view
        let _: () = msg_send![text_view, setMinSize: CGSize::new(0.0, frame.size.height)];
        let _: () = msg_send![text_view, setMaxSize: CGSize::new(f64::MAX, f64::MAX)];
        let _: () = msg_send![text_view, setVerticallyResizable: true];
        let _: () = msg_send![text_view, setHorizontallyResizable: false];

        // Text container settings
        let container: Id = msg_send![text_view, textContainer];
        let _: () = msg_send![container, setWidthTracksTextView: true];

        // Set as document view
        let _: () = msg_send![scroll, setDocumentView: text_view];

        (scroll, text_view)
    }
}

// ============================================================================
// Window Helpers
// ============================================================================

/// Create a floating overlay window
pub fn create_floating_window(frame: CGRect, title: &str, transparent_titlebar: bool) -> Id {
    unsafe {
        let ns_window = Class::get("NSWindow").unwrap();

        let style = NSWindowStyleMask::Titled
            | NSWindowStyleMask::Closable
            | NSWindowStyleMask::Miniaturizable
            | NSWindowStyleMask::Resizable;

        let window: Id = msg_send![ns_window, alloc];
        let window: Id = msg_send![
            window,
            initWithContentRect: frame
            styleMask: style
            backing: NSBackingStoreType::Buffered
            defer: false
        ];

        if transparent_titlebar {
            let _: () = msg_send![window, setTitleVisibility: 1_isize]; // NSWindowTitleHidden
            let _: () = msg_send![window, setTitlebarAppearsTransparent: true];
        }

        let _: () = msg_send![window, setMovableByWindowBackground: true];
        let _: () = msg_send![window, setLevel: NS_FLOATING_WINDOW_LEVEL];

        // Can join all spaces
        let collection = NSWindowCollectionBehavior::CanJoinAllSpaces
            | NSWindowCollectionBehavior::FullScreenAuxiliary;
        let _: () = msg_send![window, setCollectionBehavior: collection];

        if !title.is_empty() {
            let title_str = ns_string(title);
            let _: () = msg_send![window, setTitle: title_str];
        }

        window
    }
}

/// Get window's content view
pub fn window_content_view(window: Id) -> Id {
    unsafe { msg_send![window, contentView] }
}

/// Add subview to a view
pub fn add_subview(parent: Id, child: Id) {
    unsafe {
        let _: () = msg_send![parent, addSubview: child];
    }
}

/// Show window (order front)
pub fn window_show(window: Id) {
    unsafe {
        let _: () = msg_send![window, orderFrontRegardless];
    }
}

/// Close window
pub fn window_close(window: Id) {
    unsafe {
        let _: () = msg_send![window, close];
    }
}

/// Set window alpha (for fade animations)
pub fn window_set_alpha(window: Id, alpha: f64) {
    unsafe {
        let _: () = msg_send![window, setAlphaValue: alpha];
    }
}

// ============================================================================
// Segmented Control (Tab Bar) Helpers
// ============================================================================

/// Create a segmented control (tab bar)
pub fn create_segmented_control(frame: CGRect, labels: &[&str]) -> Id {
    unsafe {
        let ns_segmented = Class::get("NSSegmentedControl").unwrap();

        let control: Id = msg_send![ns_segmented, alloc];
        let control: Id = msg_send![control, initWithFrame: frame];

        let count = labels.len() as isize;
        let _: () = msg_send![control, setSegmentCount: count];

        for (i, label) in labels.iter().enumerate() {
            let label_str = ns_string(label);
            let _: () = msg_send![control, setLabel: label_str forSegment: i as isize];
        }

        let _: () = msg_send![control, setSelectedSegment: 0_isize];

        control
    }
}

// ============================================================================
// Text Field Value Helpers
// ============================================================================

/// Set text field string value
pub fn set_text(field: Id, text: &str) {
    unsafe {
        let text_str = ns_string(text);
        let _: () = msg_send![field, setStringValue: text_str];
    }
}

/// Get text field string value
pub fn get_text(field: Id) -> String {
    unsafe {
        let ns_str: Id = msg_send![field, stringValue];
        if ns_str.is_null() {
            return String::new();
        }
        let c_str: *const i8 = msg_send![ns_str, UTF8String];
        if c_str.is_null() {
            return String::new();
        }
        std::ffi::CStr::from_ptr(c_str)
            .to_string_lossy()
            .into_owned()
    }
}

/// Set text view string (for NSTextView, not NSTextField)
pub fn set_text_view_string(text_view: Id, text: &str) {
    unsafe {
        let text_str = ns_string(text);
        let _: () = msg_send![text_view, setString: text_str];
    }
}

// ============================================================================
// Animation Helpers
// ============================================================================

/// Run a simple fade animation
pub fn animate_fade(window: Id, to_alpha: f64, duration: f64) {
    unsafe {
        let ns_animation_context = Class::get("NSAnimationContext").unwrap();

        let _: () = msg_send![ns_animation_context, beginGrouping];
        let ctx: Id = msg_send![ns_animation_context, currentContext];
        let _: () = msg_send![ctx, setDuration: duration];

        let animator: Id = msg_send![window, animator];
        let _: () = msg_send![animator, setAlphaValue: to_alpha];

        let _: () = msg_send![ns_animation_context, endGrouping];
    }
}

// ============================================================================
// View Visibility Helpers
// ============================================================================

/// Set view hidden state
pub fn set_hidden(view: Id, hidden: bool) {
    unsafe {
        let _: () = msg_send![view, setHidden: hidden];
    }
}

/// Set view enabled state (for buttons)
pub fn set_enabled(view: Id, enabled: bool) {
    unsafe {
        let _: () = msg_send![view, setEnabled: enabled];
    }
}
