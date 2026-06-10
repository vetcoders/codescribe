use crate::os::clipboard;
use core_graphics::geometry::{CGPoint, CGRect, CGSize};
use objc::runtime::Class;
use objc::{msg_send, sel, sel_impl};
use std::ffi::CString;

use super::{Id, apply_tafla_surface, color_label, color_secondary_label, ui_colors, ui_tokens};

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

/// Set tooltip for a control/view.
/// # Safety
/// `view` must be a valid Objective-C object that supports `setToolTip:`.
pub unsafe fn set_tooltip(view: Id, text: &str) {
    unsafe {
        let tip = ns_string(text);
        let _: () = msg_send![view, setToolTip: tip];
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
            text_color: color_label(),
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
    pub const GLASS: isize = 16;
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

/// Set a button's SF Symbol image (returns true if applied).
/// # Safety
/// `button` must be a valid NSButton instance.
pub unsafe fn set_button_symbol(button: Id, symbol_name: &str) -> bool {
    unsafe {
        let Some(ns_image) = Class::get("NSImage") else {
            return false;
        };
        let responds: bool = msg_send![
            ns_image,
            respondsToSelector: sel!(imageWithSystemSymbolName:accessibilityDescription:)
        ];
        if !responds {
            return false;
        }
        let name = ns_string(symbol_name);
        let desc = ns_string("");
        let image: Id = msg_send![
            ns_image,
            imageWithSystemSymbolName: name
            accessibilityDescription: desc
        ];
        if image.is_null() {
            return false;
        }
        let _: () = msg_send![button, setImage: image];
        // NSImageOnly == 1
        let _: () = msg_send![button, setImagePosition: 1_isize];
        true
    }
}

/// Set an SF Symbol image on a segmented control segment.
/// # Safety
/// `control` must be a valid NSSegmentedControl instance.
pub unsafe fn set_segment_symbol(control: Id, segment: isize, symbol_name: &str) -> bool {
    let Some(ns_image) = Class::get("NSImage") else {
        return false;
    };
    let responds: bool = msg_send![
        ns_image,
        respondsToSelector: sel!(imageWithSystemSymbolName:accessibilityDescription:)
    ];
    if !responds {
        return false;
    }
    let name = ns_string(symbol_name);
    let desc = ns_string("");
    let image: Id = msg_send![
        ns_image,
        imageWithSystemSymbolName: name
        accessibilityDescription: desc
    ];
    if image.is_null() {
        return false;
    }
    let _: () = msg_send![control, setImage: image forSegment: segment];
    true
}

/// Style a toolbar icon button (borderless, inline bezel, tinted symbol).
/// # Safety
/// `button` must be a valid NSButton instance.
pub unsafe fn style_toolbar_icon_button(button: Id) {
    let _: () = msg_send![button, setBezelStyle: button_style::INLINE];
    let responds_bordered: bool = msg_send![button, respondsToSelector: sel!(setBordered:)];
    if responds_bordered {
        let _: () = msg_send![button, setBordered: false];
    }
    // NOTE: Do NOT set transparent=true — it hides the button image entirely.
    // setBordered=false + setImagePosition=NSImageOnly is sufficient for borderless icons.
    let responds_shows_border: bool =
        msg_send![button, respondsToSelector: sel!(setShowsBorderOnlyWhileMouseInside:)];
    if responds_shows_border {
        let _: () = msg_send![button, setShowsBorderOnlyWhileMouseInside: false];
    }
    let responds_image_position: bool =
        msg_send![button, respondsToSelector: sel!(setImagePosition:)];
    if responds_image_position {
        // NSImageOnly == 1
        let _: () = msg_send![button, setImagePosition: 1_isize];
    }
    let responds_tint: bool = msg_send![button, respondsToSelector: sel!(setContentTintColor:)];
    if responds_tint {
        let tint = color_label();
        let _: () = msg_send![button, setContentTintColor: tint];
    }
    let responds_control_size: bool = msg_send![button, respondsToSelector: sel!(setControlSize:)];
    if responds_control_size {
        let _: () = msg_send![button, setControlSize: 1_isize]; // NSSmallControlSize
    }
}

/// Update a toolbar icon button to reflect active/inactive tab state.
/// # Safety
/// `button` must be a valid NSButton instance.
pub unsafe fn set_tab_button_active(button: Id, active: bool) {
    let ns_color = Class::get("NSColor").unwrap();
    let responds_tint: bool = msg_send![button, respondsToSelector: sel!(setContentTintColor:)];
    if responds_tint {
        let tint: Id = if active {
            msg_send![ns_color, controlAccentColor]
        } else {
            msg_send![ns_color, secondaryLabelColor]
        };
        let _: () = msg_send![button, setContentTintColor: tint];
    }
    let responds_state: bool = msg_send![button, respondsToSelector: sel!(setState:)];
    if responds_state {
        let _: () = msg_send![button, setState: 0_isize];
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
// Toggle Helpers
// ============================================================================

/// Create a native NSSwitch toggle.
pub fn create_toggle(frame: CGRect, checked: bool) -> Id {
    unsafe {
        let ns_switch = Class::get("NSSwitch").unwrap();
        let toggle: Id = msg_send![ns_switch, alloc];
        let toggle: Id = msg_send![toggle, initWithFrame: frame];
        let state: isize = if checked { 1 } else { 0 };
        let _: () = msg_send![toggle, setState: state];
        toggle
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
        let ns_font = Class::get("NSFont").unwrap();

        let card: Id = msg_send![ns_view, alloc];
        let card: Id = msg_send![card, initWithFrame: frame];
        let _: () = msg_send![card, setWantsLayer: true];
        let layer: Id = msg_send![card, layer];
        if !layer.is_null() {
            let bg_color: Id = ui_colors::card_bg();
            let cg_color: Id = msg_send![bg_color, CGColor];
            let _: () = msg_send![layer, setBackgroundColor: cg_color];
            apply_tafla_surface(layer, true);
            // Drawer cards stay intentionally flat: one border, no halo.
            let _: () = msg_send![layer, setMasksToBounds: true];
            let _: () = msg_send![layer, setShadowRadius: 0.0f64];
            let _: () = msg_send![layer, setShadowOffset: CGSize::new(0.0, 0.0)];
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
        let title_font: Id = msg_send![ns_font, boldSystemFontOfSize: ui_tokens::BODY_FONT_SIZE];
        let _: () = msg_send![title_field, setFont: title_font];
        let title_color: Id = color_label();
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
        let subtitle_font: Id = msg_send![ns_font, systemFontOfSize: ui_tokens::SMALL_FONT_SIZE];
        let _: () = msg_send![subtitle_field, setFont: subtitle_font];
        let subtitle_color: Id = color_secondary_label();
        let _: () = msg_send![subtitle_field, setTextColor: subtitle_color];
        let _: () = msg_send![card, addSubview: subtitle_field];

        // Leave room for the actions row ("Copy / Edit / Delete / ♥") at the bottom.
        let preview_bottom = 36.0;
        let preview_top = 56.0;
        let preview_frame = CGRect::new(
            &CGPoint::new(12.0, preview_bottom),
            &CGSize::new(
                frame.size.width - 24.0,
                (frame.size.height - preview_top - preview_bottom).max(18.0),
            ),
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
        let preview_font: Id = msg_send![ns_font, systemFontOfSize: ui_tokens::BODY_FONT_SIZE];
        let _: () = msg_send![preview_field, setFont: preview_font];
        let preview_color: Id = color_secondary_label();
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
        // Keep scrolling enabled; hide scrollbars via overlay + autohide.
        let _: () = msg_send![scroll, setHasVerticalScroller: true];
        let _: () = msg_send![scroll, setHasHorizontalScroller: false];
        let _: () = msg_send![scroll, setDrawsBackground: false];
        let _: () = msg_send![scroll, setBorderType: 0_isize]; // NSNoBorder
        let _: () = msg_send![scroll, setAutohidesScrollers: true];
        // NSScrollerStyleOverlay == 1
        let _: () = msg_send![scroll, setScrollerStyle: 1_isize];

        // Create text view with same size
        let text_frame = CGRect::new(
            &CGPoint::new(0.0, 0.0),
            &CGSize::new(frame.size.width, frame.size.height),
        );
        let text_view: Id = msg_send![ns_text_view, alloc];
        let text_view: Id = msg_send![text_view, initWithFrame: text_frame];

        let _: () = msg_send![text_view, setEditable: editable];
        let _: () = msg_send![text_view, setSelectable: true];
        if editable {
            let responds_placeholder: bool =
                msg_send![text_view, respondsToSelector: sel!(setPlaceholderString:)];
            if responds_placeholder {
                let placeholder = ns_string("Type a message");
                let _: () = msg_send![text_view, setPlaceholderString: placeholder];
            }
        }

        // Transparent background
        let clear: Id = msg_send![ns_color, clearColor];
        let _: () = msg_send![text_view, setBackgroundColor: clear];

        // Dynamic text color (light/dark).
        let text_color: Id = msg_send![ns_color, textColor];
        let _: () = msg_send![text_view, setTextColor: text_color];
        let responds_caret: bool =
            msg_send![text_view, respondsToSelector: sel!(setInsertionPointColor:)];
        if responds_caret {
            let caret: Id = msg_send![ns_color, controlAccentColor];
            let _: () = msg_send![text_view, setInsertionPointColor: caret];
        }

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
