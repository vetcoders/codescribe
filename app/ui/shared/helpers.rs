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

use crate::os::clipboard;
use core_graphics::geometry::{CGPoint, CGRect, CGSize};
use objc::declare::ClassDecl;
use objc::runtime::{Class, Object, Sel};
use objc::{msg_send, sel, sel_impl};
use objc2_app_kit::{NSBackingStoreType, NSWindowCollectionBehavior, NSWindowStyleMask};
use std::ffi::CString;
use std::sync::Once;

/// Type alias for Objective-C object pointers
pub type Id = *mut Object;

/// Window level constants
pub const NS_FLOATING_WINDOW_LEVEL: i64 = 3;
pub const NS_STATUS_WINDOW_LEVEL: i64 = 25;

// Custom overlay window class so borderless windows can receive input.
static OVERLAY_WINDOW_INIT: Once = Once::new();
static mut OVERLAY_WINDOW_CLASS: *const Class = std::ptr::null();

extern "C" fn can_become_key(_this: &Object, _cmd: Sel) -> bool {
    true
}

/// Get a custom NSWindow subclass that can become key/main (for borderless overlays).
pub fn overlay_window_class() -> *const Class {
    unsafe {
        OVERLAY_WINDOW_INIT.call_once(|| {
            let superclass = Class::get("NSWindow").expect("NSWindow not found");
            let mut decl = ClassDecl::new("CodeScribeOverlayWindow", superclass)
                .expect("Failed to declare overlay window class");
            decl.add_method(
                sel!(canBecomeKeyWindow),
                can_become_key as extern "C" fn(&Object, Sel) -> bool,
            );
            decl.add_method(
                sel!(canBecomeMainWindow),
                can_become_key as extern "C" fn(&Object, Sel) -> bool,
            );
            OVERLAY_WINDOW_CLASS = decl.register();
        });
        OVERLAY_WINDOW_CLASS
    }
}

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

/// Copy text to the system clipboard (best-effort).
pub fn copy_to_clipboard(text: &str) {
    let _ = clipboard::copy(text);
}

/// Set a tooltip on any NSView.
/// # Safety
/// `view` must be a valid Objective-C object that supports `setToolTip:`.
pub unsafe fn set_tooltip(view: Id, text: &str) {
    unsafe {
        let tip = ns_string(text);
        let _: () = msg_send![view, setToolTip: tip];
    }
}

// ============================================================================
// Text Field Helpers
// ============================================================================

/// Get string value from an NSTextField/NSSearchField
/// # Safety
/// `field` must be a valid `NSTextField`/`NSSearchField` instance.
pub unsafe fn get_text_field_string(field: Id) -> String {
    unsafe {
        let value: Id = msg_send![field, stringValue];
        let c_str: *const i8 = msg_send![value, UTF8String];
        if c_str.is_null() {
            return String::new();
        }
        std::ffi::CStr::from_ptr(c_str)
            .to_string_lossy()
            .to_string()
    }
}

/// Set string value for an NSTextField/NSSearchField
/// # Safety
/// `field` must be a valid `NSTextField`/`NSSearchField` instance.
pub unsafe fn set_text_field_string(field: Id, text: &str) {
    unsafe {
        let value = ns_string(text);
        let _: () = msg_send![field, setStringValue: value];
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

/// Create a search field
pub fn create_search_field(frame: CGRect, placeholder: &str) -> Id {
    unsafe {
        let ns_search_field = Class::get("NSSearchField").unwrap();
        let field: Id = msg_send![ns_search_field, alloc];
        let field: Id = msg_send![field, initWithFrame: frame];
        let placeholder = ns_string(placeholder);
        let _: () = msg_send![field, setPlaceholderString: placeholder];
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
// Segmented Control Helpers
// ============================================================================

/// Create a segmented control with labels
pub fn create_segmented_control(frame: CGRect, labels: &[&str]) -> Id {
    unsafe {
        let ns_segmented = Class::get("NSSegmentedControl").unwrap();
        let control: Id = msg_send![ns_segmented, alloc];
        let control: Id = msg_send![control, initWithFrame: frame];
        let _: () = msg_send![control, setSegmentCount: labels.len() as isize];
        for (idx, label) in labels.iter().enumerate() {
            let title = ns_string(label);
            let _: () = msg_send![control, setLabel: title forSegment: idx as isize];
        }
        let _: () = msg_send![control, setSelectedSegment: 0_isize];
        control
    }
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
/// # Safety
/// `button` and `target` must be valid Objective-C objects.
pub unsafe fn button_set_action(button: Id, target: Id, action: objc::runtime::Sel) {
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
// Card Helpers
// ============================================================================

/// Create a drawer card view with a title, subtitle, and preview text
pub fn create_card_view(frame: CGRect, title: &str, subtitle: &str, preview: &str) -> Id {
    unsafe {
        let ns_view = Class::get("NSView").unwrap();
        let ns_text_field = Class::get("NSTextField").unwrap();
        let ns_color = Class::get("NSColor").unwrap();
        let ns_font = Class::get("NSFont").unwrap();

        let card: Id = msg_send![ns_view, alloc];
        let card: Id = msg_send![card, initWithFrame: frame];
        let _: () = msg_send![card, setWantsLayer: true];
        let layer: Id = msg_send![card, layer];
        if !layer.is_null() {
            let bg_color: Id =
                msg_send![ns_color, colorWithRed: 0.2 green: 0.2 blue: 0.2 alpha: 0.4];
            let cg_color: Id = msg_send![bg_color, CGColor];
            let _: () = msg_send![layer, setBackgroundColor: cg_color];
            let _: () = msg_send![layer, setCornerRadius: 12.0f64];
        }

        let title_frame = CGRect::new(
            &CGPoint::new(12.0, frame.size.height - 24.0),
            &CGSize::new(frame.size.width - 24.0, 18.0),
        );
        let title_field: Id = msg_send![ns_text_field, alloc];
        let title_field: Id = msg_send![title_field, initWithFrame: title_frame];
        let _: () = msg_send![title_field, setBezeled: false];
        let _: () = msg_send![title_field, setDrawsBackground: false];
        let _: () = msg_send![title_field, setEditable: false];
        let _: () = msg_send![title_field, setSelectable: false];
        let title_text = ns_string(title);
        let _: () = msg_send![title_field, setStringValue: title_text];
        let title_font: Id = msg_send![ns_font, boldSystemFontOfSize: 12.0f64];
        let _: () = msg_send![title_field, setFont: title_font];
        let title_color: Id = msg_send![ns_color, colorWithWhite: 1.0 alpha: 0.9];
        let _: () = msg_send![title_field, setTextColor: title_color];
        let _: () = msg_send![card, addSubview: title_field];

        let subtitle_frame = CGRect::new(
            &CGPoint::new(12.0, frame.size.height - 42.0),
            &CGSize::new(frame.size.width - 24.0, 16.0),
        );
        let subtitle_field: Id = msg_send![ns_text_field, alloc];
        let subtitle_field: Id = msg_send![subtitle_field, initWithFrame: subtitle_frame];
        let _: () = msg_send![subtitle_field, setBezeled: false];
        let _: () = msg_send![subtitle_field, setDrawsBackground: false];
        let _: () = msg_send![subtitle_field, setEditable: false];
        let _: () = msg_send![subtitle_field, setSelectable: false];
        let subtitle_text = ns_string(subtitle);
        let _: () = msg_send![subtitle_field, setStringValue: subtitle_text];
        let subtitle_font: Id = msg_send![ns_font, systemFontOfSize: 11.0f64];
        let _: () = msg_send![subtitle_field, setFont: subtitle_font];
        let subtitle_color: Id = msg_send![ns_color, colorWithWhite: 1.0 alpha: 0.5];
        let _: () = msg_send![subtitle_field, setTextColor: subtitle_color];
        let _: () = msg_send![card, addSubview: subtitle_field];

        let preview_frame = CGRect::new(
            &CGPoint::new(12.0, 12.0),
            &CGSize::new(frame.size.width - 24.0, frame.size.height - 56.0),
        );
        let preview_field: Id = msg_send![ns_text_field, alloc];
        let preview_field: Id = msg_send![preview_field, initWithFrame: preview_frame];
        let _: () = msg_send![preview_field, setBezeled: false];
        let _: () = msg_send![preview_field, setDrawsBackground: false];
        let _: () = msg_send![preview_field, setEditable: false];
        let _: () = msg_send![preview_field, setSelectable: false];
        let _: () = msg_send![preview_field, setLineBreakMode: 0];
        let preview_text = ns_string(preview);
        let _: () = msg_send![preview_field, setStringValue: preview_text];
        let preview_font: Id = msg_send![ns_font, systemFontOfSize: 12.0f64];
        let _: () = msg_send![preview_field, setFont: preview_font];
        let preview_color: Id = msg_send![ns_color, colorWithWhite: 1.0 alpha: 0.85];
        let _: () = msg_send![preview_field, setTextColor: preview_color];
        let _: () = msg_send![card, addSubview: preview_field];

        card
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
/// # Safety
/// `window` must be a valid `NSWindow` instance.
pub unsafe fn window_content_view(window: Id) -> Id {
    unsafe { msg_send![window, contentView] }
}

/// Add subview to a view
/// # Safety
/// `parent` and `child` must be valid Objective-C views.
pub unsafe fn add_subview(parent: Id, child: Id) {
    unsafe {
        let _: () = msg_send![parent, addSubview: child];
    }
}

/// Show window (order front)
/// # Safety
/// `window` must be a valid `NSWindow` instance.
pub unsafe fn window_show(window: Id) {
    unsafe {
        let _: () = msg_send![window, orderFrontRegardless];
    }
}

/// Close window
/// # Safety
/// `window` must be a valid `NSWindow` instance.
pub unsafe fn window_close(window: Id) {
    unsafe {
        let _: () = msg_send![window, close];
    }
}

/// Set window alpha (for fade animations)
/// # Safety
/// `window` must be a valid `NSWindow` instance.
pub unsafe fn window_set_alpha(window: Id, alpha: f64) {
    unsafe {
        let _: () = msg_send![window, setAlphaValue: alpha];
    }
}

#[cfg(test)]
mod tests {
    use super::{CGPoint, CGRect, CGSize, clamp_overlay_position};

    #[test]
    fn clamp_overlay_position_keeps_window_inside_frame() {
        let visible = CGRect::new(&CGPoint::new(0.0, 0.0), &CGSize::new(100.0, 100.0));
        let (x, y) = clamp_overlay_position(visible, 60.0, 60.0, 10.0, 1000.0, -1000.0);
        assert_eq!(x, 30.0);
        assert_eq!(y, 10.0);
    }
}

/// Clamp overlay position to visible frame with margin.
pub fn clamp_overlay_position(
    visible_frame: CGRect,
    window_width: f64,
    window_height: f64,
    margin: f64,
    raw_x: f64,
    raw_y: f64,
) -> (f64, f64) {
    let min_x = visible_frame.origin.x + margin;
    let max_x = visible_frame.origin.x + visible_frame.size.width - window_width - margin;
    let min_y = visible_frame.origin.y + margin;
    let max_y = visible_frame.origin.y + visible_frame.size.height - window_height - margin;

    let x = raw_x.clamp(min_x, max_x.max(min_x));
    let y = raw_y.clamp(min_y, max_y.max(min_y));
    (x, y)
}

// ============================================================================
// Text Field Value Helpers
// ============================================================================

/// Set text field string value
/// # Safety
/// `field` must be a valid `NSTextField` (or compatible) instance.
pub unsafe fn set_text(field: Id, text: &str) {
    unsafe {
        let text_str = ns_string(text);
        let _: () = msg_send![field, setStringValue: text_str];
    }
}

/// Get text field string value
/// # Safety
/// `field` must be a valid `NSTextField` (or compatible) instance.
pub unsafe fn get_text(field: Id) -> String {
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
/// # Safety
/// `text_view` must be a valid `NSTextView` instance.
pub unsafe fn set_text_view_string(text_view: Id, text: &str) {
    unsafe {
        let text_str = ns_string(text);
        let _: () = msg_send![text_view, setString: text_str];
    }
}

/// Get text view string (for NSTextView)
/// # Safety
/// `text_view` must be a valid `NSTextView` instance.
pub unsafe fn get_text_view_string(text_view: Id) -> String {
    unsafe {
        let ns_str: Id = msg_send![text_view, string];
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

// ============================================================================
// Animation Helpers
// ============================================================================

/// Run a simple fade animation
/// # Safety
/// `window` must be a valid `NSWindow` instance.
pub unsafe fn animate_fade(window: Id, to_alpha: f64, duration: f64) {
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

/// Animate window width change (horizontal slide for drawer collapse)
/// # Safety
/// `window` must be a valid `NSWindow` instance.
pub unsafe fn animate_window_width(window: Id, to_width: f64, duration: f64) {
    unsafe {
        let ns_animation_context = Class::get("NSAnimationContext").unwrap();

        // Get current frame
        let current_frame: CGRect = msg_send![window, frame];

        // Calculate new frame with same origin and height but new width
        let new_frame = CGRect::new(
            &current_frame.origin,
            &CGSize::new(to_width, current_frame.size.height),
        );

        let _: () = msg_send![ns_animation_context, beginGrouping];
        let ctx: Id = msg_send![ns_animation_context, currentContext];
        let _: () = msg_send![ctx, setDuration: duration];

        // Animate frame change
        let animator: Id = msg_send![window, animator];
        let _: () = msg_send![animator, setFrame: new_frame display: true];

        let _: () = msg_send![ns_animation_context, endGrouping];
    }
}

// ============================================================================
// View Visibility Helpers
// ============================================================================

/// Set view hidden state
/// # Safety
/// `view` must be a valid `NSView` (or subclass) instance.
pub unsafe fn set_hidden(view: Id, hidden: bool) {
    unsafe {
        let _: () = msg_send![view, setHidden: hidden];
    }
}

/// Set view enabled state (for buttons)
/// # Safety
/// `view` must be a valid `NSView`/`NSControl` instance.
pub unsafe fn set_enabled(view: Id, enabled: bool) {
    unsafe {
        let _: () = msg_send![view, setEnabled: enabled];
    }
}

// ============================================================================
// Chat Bubble Helpers (GlyphPulse / Quantum style)
// ============================================================================

/// GlyphPulse/Quantum palette adapted for native bubbles
pub mod bubble_colors {
    /// User bubble background - quantum cyan
    pub const USER_BG: (f64, f64, f64, f64) = (0.0, 1.0, 1.0, 1.0);
    /// User bubble text - CRT black
    pub const USER_TEXT: (f64, f64, f64, f64) = (0.039, 0.039, 0.039, 1.0);
    /// Assistant bubble background - deep navy glass
    pub const ASSISTANT_BG: (f64, f64, f64, f64) = (0.086, 0.129, 0.243, 0.5);
    /// Assistant bubble border - subtle white
    pub const ASSISTANT_BORDER: (f64, f64, f64, f64) = (1.0, 1.0, 1.0, 0.08);
    /// Assistant/system text - primary light
    pub const ASSISTANT_TEXT: (f64, f64, f64, f64) = (0.886, 0.91, 0.941, 1.0);
    /// Streaming text tint - muted
    pub const STREAMING_TEXT: (f64, f64, f64, f64) = (0.58, 0.639, 0.722, 1.0);
    /// System bubble background - slightly denser navy
    pub const SYSTEM_BG: (f64, f64, f64, f64) = (0.086, 0.129, 0.243, 0.65);
    /// System bubble border - subtle white
    pub const SYSTEM_BORDER: (f64, f64, f64, f64) = (1.0, 1.0, 1.0, 0.08);
    /// Error bubble background - soft red tint
    pub const ERROR_BG: (f64, f64, f64, f64) = (1.0, 0.42, 0.42, 0.1);
    /// Error bubble border/text - error red
    pub const ERROR_BORDER: (f64, f64, f64, f64) = (1.0, 0.373, 0.341, 1.0);
    pub const ERROR_TEXT: (f64, f64, f64, f64) = (1.0, 0.373, 0.341, 1.0);
}

/// Role for chat bubble styling
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BubbleRole {
    User,
    Assistant,
    System,
}

/// Configuration for creating a chat bubble
pub struct BubbleConfig {
    pub text: String,
    pub role: BubbleRole,
    pub max_width: f64,
    pub is_streaming: bool,
    pub is_error: bool,
    /// Optional message index for Copy button (None = no button)
    pub message_index: Option<usize>,
    /// Optional action target for Copy button
    pub copy_action_target: Option<Id>,
}

/// Create a chat bubble view (NSView container with styled text)
///
/// Returns (container_view, text_label) tuple for later updates
pub fn create_bubble_view(config: BubbleConfig) -> (Id, Id) {
    unsafe {
        let ns_view = Class::get("NSView").unwrap();
        let ns_text_field = Class::get("NSTextField").unwrap();
        let ns_color = Class::get("NSColor").unwrap();
        let ns_font = Class::get("NSFont").unwrap();
        let ns_dict = Class::get("NSDictionary").unwrap();

        let font_size = 13.0;
        let padding_x = 12.0;
        let padding_top = 10.0;
        let copy_button_height = if config.message_index.is_some() {
            16.0
        } else {
            0.0
        };
        // Reserve space for the Copy button so it never overlaps text.
        let padding_bottom = if copy_button_height > 0.0 {
            copy_button_height + 8.0
        } else {
            10.0
        };
        let line_height = font_size * 1.4;

        // Font (prefer JetBrains Mono if installed)
        let jb_name = ns_string("JetBrainsMono-Regular");
        let jb_font: Id = msg_send![ns_font, fontWithName: jb_name size: font_size];
        let font: Id = if jb_font.is_null() {
            msg_send![ns_font, monospacedSystemFontOfSize: font_size weight: 0.0f64]
        } else {
            jb_font
        };

        // Set text (with streaming indicator if needed)
        let display_text = if config.is_streaming && config.text.is_empty() {
            "• • •".to_string() // Pulsing dots placeholder
        } else if config.is_streaming {
            format!("{} …", config.text)
        } else {
            config.text.clone()
        };

        // Measure text height/width using NSString boundingRectWithSize (handles newlines/wrapping).
        //
        // NOTE: `NSFontAttributeName` (key) has the string value "NSFont". AppKit expects that
        // key, not the literal "NSFontAttributeName" string.
        let text_str = ns_string(&display_text);
        let font_key = ns_string("NSFont");
        let attrs: Id = msg_send![ns_dict, dictionaryWithObject: font forKey: font_key];
        let opts: u64 = 1 | 2; // NSStringDrawingUsesLineFragmentOrigin | NSStringDrawingUsesFontLeading

        // Keep a small side margin inside the container so full-width bubbles don't overflow.
        let bubble_max_width = (config.max_width - 16.0).max(80.0);
        let text_max_width = (bubble_max_width - padding_x * 2.0).max(40.0);
        let rect_max: CGRect = msg_send![
            text_str,
            boundingRectWithSize: CGSize::new(text_max_width, 10_000.0)
            options: opts
            attributes: attrs
        ];

        // Bubble width: content-aware but capped.
        // If it wraps (or is long), keep the bubble full width for readability.
        //
        // We treat streaming messages as "wrap-prone" earlier to avoid the initial narrow bubble
        // that later expands mid-stream.
        let long_threshold = if config.is_streaming { 30 } else { 80 };
        let is_long = display_text.chars().count() > long_threshold;
        let wraps_at_max =
            rect_max.size.height > line_height * 1.6 || display_text.contains('\n') || is_long;
        let bubble_width = if wraps_at_max {
            bubble_max_width
        } else {
            let content_width = rect_max.size.width.min(text_max_width).max(1.0);
            (content_width + padding_x * 2.0).min(bubble_max_width)
        };

        // Re-measure height for the final layout width (important when bubble_width < max).
        let text_layout_width = (bubble_width - padding_x * 2.0).max(40.0);
        let text_rect: CGRect = msg_send![
            text_str,
            boundingRectWithSize: CGSize::new(text_layout_width, 10_000.0)
            options: opts
            attributes: attrs
        ];
        let text_height = text_rect.size.height.ceil().max(line_height);
        let bubble_height = text_height + padding_top + padding_bottom;

        // Container view (for alignment)
        let container: Id = msg_send![ns_view, alloc];
        let container_frame = CGRect::new(
            &CGPoint::new(0.0, 0.0),
            &CGSize::new(config.max_width, bubble_height),
        );
        let container: Id = msg_send![container, initWithFrame: container_frame];

        // Bubble background view
        let bubble: Id = msg_send![ns_view, alloc];
        let bubble_x = match config.role {
            BubbleRole::User => (config.max_width - bubble_width - 8.0).max(8.0), // Right-aligned
            BubbleRole::Assistant | BubbleRole::System => 8.0,                    // Left-aligned
        };
        let bubble_frame = CGRect::new(
            &CGPoint::new(bubble_x, 0.0),
            &CGSize::new(bubble_width, bubble_height),
        );
        let bubble: Id = msg_send![bubble, initWithFrame: bubble_frame];

        // Set bubble background color based on role
        let (r, g, b, a) = if config.is_error {
            bubble_colors::ERROR_BG
        } else {
            match config.role {
                BubbleRole::User => bubble_colors::USER_BG,
                BubbleRole::Assistant => bubble_colors::ASSISTANT_BG,
                BubbleRole::System => bubble_colors::SYSTEM_BG,
            }
        };
        let bg_color: Id = msg_send![ns_color, colorWithRed: r green: g blue: b alpha: a];

        // Set background via layer (for rounded corners)
        let _: () = msg_send![bubble, setWantsLayer: true];
        let layer: Id = msg_send![bubble, layer];
        if !layer.is_null() {
            // CGColor from NSColor
            let cg_color: Id = msg_send![bg_color, CGColor];
            let _: () = msg_send![layer, setBackgroundColor: cg_color];
            let _: () = msg_send![layer, setCornerRadius: 12.0f64];
            let _: () = msg_send![layer, setMasksToBounds: false];
            // Border styling
            let (br, bg, bb, ba, bw) = if config.is_error {
                (
                    bubble_colors::ERROR_BORDER.0,
                    bubble_colors::ERROR_BORDER.1,
                    bubble_colors::ERROR_BORDER.2,
                    bubble_colors::ERROR_BORDER.3,
                    1.0f64,
                )
            } else {
                match config.role {
                    BubbleRole::Assistant => (
                        bubble_colors::ASSISTANT_BORDER.0,
                        bubble_colors::ASSISTANT_BORDER.1,
                        bubble_colors::ASSISTANT_BORDER.2,
                        bubble_colors::ASSISTANT_BORDER.3,
                        1.0f64,
                    ),
                    BubbleRole::System => (
                        bubble_colors::SYSTEM_BORDER.0,
                        bubble_colors::SYSTEM_BORDER.1,
                        bubble_colors::SYSTEM_BORDER.2,
                        bubble_colors::SYSTEM_BORDER.3,
                        1.0f64,
                    ),
                    BubbleRole::User => (0.0, 0.0, 0.0, 0.0, 0.0f64),
                }
            };
            if bw > 0.0 {
                let border_color: Id =
                    msg_send![ns_color, colorWithRed: br green: bg blue: bb alpha: ba];
                let cg_border: Id = msg_send![border_color, CGColor];
                let _: () = msg_send![layer, setBorderColor: cg_border];
                let _: () = msg_send![layer, setBorderWidth: bw];
            }
        }

        // Text label inside bubble
        let text_frame = CGRect::new(
            &CGPoint::new(padding_x, padding_bottom),
            &CGSize::new((bubble_width - padding_x * 2.0).max(1.0), text_height),
        );
        let text_label: Id = msg_send![ns_text_field, alloc];
        let text_label: Id = msg_send![text_label, initWithFrame: text_frame];

        let _: () = msg_send![text_label, setBezeled: false];
        let _: () = msg_send![text_label, setEditable: false];
        let _: () = msg_send![text_label, setSelectable: true];
        let _: () = msg_send![text_label, setDrawsBackground: false];

        // Text color (role-aware)
        let (tr, tg, tb, ta) = if config.is_error {
            bubble_colors::ERROR_TEXT
        } else {
            match config.role {
                BubbleRole::User => bubble_colors::USER_TEXT,
                BubbleRole::Assistant => {
                    if config.is_streaming {
                        bubble_colors::STREAMING_TEXT
                    } else {
                        bubble_colors::ASSISTANT_TEXT
                    }
                }
                BubbleRole::System => bubble_colors::ASSISTANT_TEXT,
            }
        };
        let text_color: Id = msg_send![ns_color, colorWithRed: tr green: tg blue: tb alpha: ta];
        let _: () = msg_send![text_label, setTextColor: text_color];

        let _: () = msg_send![text_label, setFont: font];

        let _: () = msg_send![text_label, setStringValue: text_str];

        // Word wrap
        let _: () = msg_send![text_label, setLineBreakMode: 0_isize]; // NSLineBreakByWordWrapping

        // Assemble hierarchy
        let _: () = msg_send![bubble, addSubview: text_label];

        // Add Copy button if message_index is provided
        if let (Some(msg_index), Some(target)) = (config.message_index, config.copy_action_target) {
            let ns_button = Class::get("NSButton").unwrap();

            let button_width = 40.0;
            let button_height = copy_button_height;
            let button_x = bubble_width - button_width - padding_x;
            let button_y = 4.0; // Bottom of bubble

            let button_frame = CGRect::new(
                &CGPoint::new(button_x, button_y),
                &CGSize::new(button_width, button_height),
            );

            let copy_button: Id = msg_send![ns_button, alloc];
            let copy_button: Id = msg_send![copy_button, initWithFrame: button_frame];

            // Style: small borderless button
            let _: () = msg_send![copy_button, setBezelStyle: 0_isize]; // NSBezelStyleRounded
            let _: () = msg_send![copy_button, setBordered: false];

            // Title "Copy" in small font
            let title = ns_string("Copy");
            let _: () = msg_send![copy_button, setTitle: title];

            let small_font: Id = if jb_font.is_null() {
                msg_send![ns_font, monospacedSystemFontOfSize: 10.0f64 weight: 0.0f64]
            } else {
                msg_send![ns_font, fontWithName: jb_name size: 10.0f64]
            };
            let _: () = msg_send![copy_button, setFont: small_font];

            // Match bubble text tint
            let button_color: Id =
                msg_send![ns_color, colorWithRed: tr green: tg blue: tb alpha: ta];
            let _: () = msg_send![copy_button, setContentTintColor: button_color];

            // Store message index in tag for retrieval on click
            let _: () = msg_send![copy_button, setTag: msg_index as isize];

            // Set action
            let _: () = msg_send![copy_button, setTarget: target];
            let _: () = msg_send![copy_button, setAction: sel!(onCopyMessage:)];

            let _: () = msg_send![bubble, addSubview: copy_button];
        }

        let _: () = msg_send![container, addSubview: bubble];

        (container, text_label)
    }
}

/// Update bubble text (for streaming updates)
/// # Safety
/// `text_label` must be a valid `NSTextField` instance.
pub unsafe fn update_bubble_text(text_label: Id, text: &str, is_streaming: bool) {
    unsafe {
        let ns_color = Class::get("NSColor").unwrap();

        let display_text = if is_streaming && text.is_empty() {
            "• • •".to_string()
        } else if is_streaming {
            format!("{} …", text)
        } else {
            text.to_string()
        };

        let text_str = ns_string(&display_text);
        let _: () = msg_send![text_label, setStringValue: text_str];

        // Update text color based on streaming state (assistant defaults)
        let (tr, tg, tb, ta) = if is_streaming {
            bubble_colors::STREAMING_TEXT
        } else {
            bubble_colors::ASSISTANT_TEXT
        };
        let text_color: Id = msg_send![ns_color, colorWithRed: tr green: tg blue: tb alpha: ta];
        let _: () = msg_send![text_label, setTextColor: text_color];
    }
}

/// Update a stack view item (bubble container) height constraint if present.
///
/// `stack_view_add` installs a fixed-height constraint on each arranged subview.
/// During streaming, the bubble text grows and we need to update that constraint
/// so the view doesn't clip.
///
/// # Safety
/// `view` must be a valid `NSView` instance.
pub unsafe fn update_stack_item_height(view: Id, new_height: f64) {
    unsafe {
        let constraints: Id = msg_send![view, constraints];
        if constraints.is_null() {
            return;
        }
        let count: usize = msg_send![constraints, count];
        for i in 0..count {
            let c: Id = msg_send![constraints, objectAtIndex: i];
            if c.is_null() {
                continue;
            }

            // Prefer our tagged constraint.
            let ident: Id = msg_send![c, identifier];
            if !ident.is_null() {
                let c_str: *const i8 = msg_send![ident, UTF8String];
                if !c_str.is_null() {
                    let s = std::ffi::CStr::from_ptr(c_str).to_string_lossy();
                    if s == "codescribe_height" {
                        let _: () = msg_send![c, setConstant: new_height];
                        return;
                    }
                }
            }

            // Fallback: find a height constraint on this view.
            let first: Id = msg_send![c, firstItem];
            if first != view {
                continue;
            }
            let second: Id = msg_send![c, secondItem];
            if !second.is_null() {
                continue;
            }
            let first_attr: isize = msg_send![c, firstAttribute];
            // NSLayoutAttributeHeight == 8
            if first_attr == 8 {
                let _: () = msg_send![c, setConstant: new_height];
                return;
            }
        }
    }
}

/// Resize an existing bubble container + its internal views for the given text.
///
/// Used for streaming updates to prevent clipping without rebuilding the whole view tree.
///
/// # Safety
/// `container` must be the container returned by `create_bubble_view`.
/// `text_label` must be the label returned by `create_bubble_view`.
pub unsafe fn resize_bubble_container_for_text(container: Id, text_label: Id, display_text: &str) {
    unsafe {
        let ns_dict = Class::get("NSDictionary").unwrap();
        let ns_font = Class::get("NSFont").unwrap();

        let font: Id = msg_send![text_label, font];
        let font = if font.is_null() {
            msg_send![ns_font, systemFontOfSize: 13.0f64]
        } else {
            font
        };

        let container_frame: CGRect = msg_send![container, frame];
        let max_width = container_frame.size.width.max(80.0);
        let bubble_max_width = (max_width - 16.0).max(80.0);

        // If the message is getting long, switch to full-width to avoid one-word-per-line bubbles.
        //
        // During streaming we append " …" so we can detect it and widen earlier to prevent
        // the initial narrow bubble phase.
        let streaming_like = display_text.ends_with('…');
        let long_threshold = if streaming_like { 30 } else { 80 };
        let is_long = display_text.chars().count() > long_threshold;
        let force_full_width = display_text.contains('\n') || is_long;

        let label_frame: CGRect = msg_send![text_label, frame];
        let width = if force_full_width {
            let padding_x = 12.0;
            (bubble_max_width - padding_x * 2.0).max(40.0)
        } else {
            label_frame.size.width.max(1.0)
        };

        let text_str = ns_string(display_text);
        let font_key = ns_string("NSFont");
        let attrs: Id = msg_send![ns_dict, dictionaryWithObject: font forKey: font_key];
        let opts: u64 = 1 | 2; // NSStringDrawingUsesLineFragmentOrigin | NSStringDrawingUsesFontLeading
        let text_rect: CGRect = msg_send![
            text_str,
            boundingRectWithSize: CGSize::new(width, 10_000.0)
            options: opts
            attributes: attrs
        ];

        // Approximate line-height floor to avoid tiny/bad measurements.
        let point_size: f64 = msg_send![font, pointSize];
        let line_height = (point_size * 1.35).max(14.0);

        let text_height = text_rect.size.height.ceil().max(line_height);

        // Match `create_bubble_view` layout constants.
        let padding_top = 10.0;
        let copy_button_height = 16.0;
        let padding_bottom = copy_button_height + 8.0;
        let bubble_height = text_height + padding_top + padding_bottom;

        // Resize bubble background view (label's superview).
        let bubble: Id = msg_send![text_label, superview];
        if !bubble.is_null() {
            let bubble_frame: CGRect = msg_send![bubble, frame];
            let mut bubble_width = bubble_frame.size.width;
            let mut bubble_x = bubble_frame.origin.x;

            if force_full_width {
                bubble_width = bubble_max_width;
                // Preserve alignment based on prior x (user bubbles are right-aligned).
                let was_right_aligned = bubble_x > 20.0;
                bubble_x = if was_right_aligned {
                    (max_width - bubble_width - 8.0).max(8.0)
                } else {
                    8.0
                };
            }

            // Resize label to match bubble width (keep in sync with create_bubble_view).
            let padding_x = 12.0;
            let new_label_w = (bubble_width - padding_x * 2.0).max(1.0);
            let new_label_frame = CGRect::new(
                &CGPoint::new(padding_x, padding_bottom),
                &CGSize::new(new_label_w, text_height),
            );
            let _: () = msg_send![text_label, setFrame: new_label_frame];

            let new_bubble_frame = CGRect::new(
                &CGPoint::new(bubble_x, bubble_frame.origin.y),
                &CGSize::new(bubble_width, bubble_height),
            );
            let _: () = msg_send![bubble, setFrame: new_bubble_frame];
            let _: () = msg_send![bubble, setNeedsDisplay: true];
        }

        // Resize container (stack arranged subview).
        let _: () = msg_send![container, setFrameSize: CGSize::new(container_frame.size.width, bubble_height)];
        update_stack_item_height(container, bubble_height);

        let _: () = msg_send![container, setNeedsLayout: true];
        let _: () = msg_send![container, layoutSubtreeIfNeeded];
        let _: () = msg_send![container, setNeedsDisplay: true];
    }
}

// ============================================================================
// File Operations Helpers
// ============================================================================

/// Open a file in the default editor (TextEdit, etc.)
pub fn open_file_in_editor(path: &std::path::Path) -> bool {
    unsafe {
        let ns_workspace = Class::get("NSWorkspace").unwrap();
        let workspace: Id = msg_send![ns_workspace, sharedWorkspace];

        let path_str = path.to_string_lossy();
        let ns_path = ns_string(&path_str);

        let result: bool = msg_send![workspace, openFile: ns_path];
        result
    }
}

/// List draft files from a directory, sorted by modification time (newest first)
pub fn list_draft_files(dir: &std::path::Path) -> Vec<std::path::PathBuf> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };

    let mut files: Vec<(std::path::PathBuf, std::time::SystemTime)> = entries
        .flatten()
        .filter(|e| {
            e.path().is_file()
                && e.path()
                    .extension()
                    .is_some_and(|ext| ext == "txt" || ext == "md")
        })
        .filter_map(|e| {
            let path = e.path();
            let modified = e.metadata().ok()?.modified().ok()?;
            Some((path, modified))
        })
        .collect();

    // Sort by modification time, newest first
    files.sort_by(|a, b| b.1.cmp(&a.1));

    files.into_iter().map(|(path, _)| path).collect()
}

// ============================================================================
// NSStackView Helpers
// ============================================================================

/// Create a vertical NSStackView for stacking views
pub fn create_vertical_stack_view(frame: CGRect) -> Id {
    unsafe {
        let ns_stack_view = Class::get("NSStackView").unwrap();

        let stack: Id = msg_send![ns_stack_view, alloc];
        let stack: Id = msg_send![stack, initWithFrame: frame];

        // Vertical orientation (1 = NSUserInterfaceLayoutOrientationVertical)
        let _: () = msg_send![stack, setOrientation: 1_isize];
        // Top alignment
        let _: () = msg_send![stack, setAlignment: 1_isize]; // NSLayoutAttributeLeft
        // Spacing between views
        let _: () = msg_send![stack, setSpacing: 8.0f64];

        stack
    }
}

/// Add a view to NSStackView
/// # Safety
/// `stack` must be a valid `NSStackView` and `view` a valid `NSView`.
pub unsafe fn stack_view_add(stack: Id, view: Id) {
    unsafe {
        let _: () = msg_send![stack, addArrangedSubview: view];

        // Pin height to the initial frame height (good enough for our chat bubbles/cards).
        let frame: CGRect = msg_send![view, frame];
        let height_anchor: Id = msg_send![view, heightAnchor];
        let height_constraint: Id =
            msg_send![height_anchor, constraintEqualToConstant: frame.size.height];
        // Tag for later updates (streaming bubbles grow).
        let _: () = msg_send![height_constraint, setIdentifier: ns_string("codescribe_height")];
        let _: () = msg_send![height_constraint, setActive: true];
    }
}

/// Remove all views from NSStackView
/// # Safety
/// `stack` must be a valid `NSStackView` instance.
pub unsafe fn stack_view_clear(stack: Id) {
    unsafe {
        let arranged: Id = msg_send![stack, arrangedSubviews];
        let count: usize = msg_send![arranged, count];

        for i in (0..count).rev() {
            let view: Id = msg_send![arranged, objectAtIndex: i];
            let _: () = msg_send![view, removeFromSuperview];
        }
    }
}
