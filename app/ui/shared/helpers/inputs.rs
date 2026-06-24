use core_graphics::geometry::CGRect;
use objc::runtime::Class;
use objc::{msg_send, sel, sel_impl};

use super::{Id, ns_string};

/// Create an editable text input field with a border and placeholder.
pub fn create_text_input(frame: CGRect, placeholder: &str, initial_value: &str) -> Id {
    unsafe {
        let ns_text_field = Class::get("NSTextField").unwrap();
        let ns_font = Class::get("NSFont").unwrap();

        let field: Id = msg_send![ns_text_field, alloc];
        let field: Id = msg_send![field, initWithFrame: frame];

        let _: () = msg_send![field, setBezeled: true];
        let _: () = msg_send![field, setEditable: true];
        let _: () = msg_send![field, setSelectable: true];
        let _: () = msg_send![field, setDrawsBackground: true];

        let font: Id = msg_send![ns_font, systemFontOfSize: 13.0f64];
        let _: () = msg_send![field, setFont: font];

        let ph = ns_string(placeholder);
        let _: () = msg_send![field, setPlaceholderString: ph];

        if !initial_value.is_empty() {
            let val = ns_string(initial_value);
            let _: () = msg_send![field, setStringValue: val];
        }

        field
    }
}

/// Create a secure (password) text input field.
pub fn create_secure_text_input(frame: CGRect, placeholder: &str) -> Id {
    unsafe {
        let ns_secure = Class::get("NSSecureTextField").unwrap();
        let ns_font = Class::get("NSFont").unwrap();

        let field: Id = msg_send![ns_secure, alloc];
        let field: Id = msg_send![field, initWithFrame: frame];

        let _: () = msg_send![field, setBezeled: true];
        let _: () = msg_send![field, setEditable: true];
        let _: () = msg_send![field, setSelectable: true];
        let _: () = msg_send![field, setDrawsBackground: true];

        let font: Id = msg_send![ns_font, systemFontOfSize: 13.0f64];
        let _: () = msg_send![field, setFont: font];

        let ph = ns_string(placeholder);
        let _: () = msg_send![field, setPlaceholderString: ph];

        field
    }
}

/// Create an NSSlider (continuous, horizontal).
pub fn create_slider(frame: CGRect, min: f64, max: f64, value: f64) -> Id {
    unsafe {
        let ns_slider = Class::get("NSSlider").unwrap();

        let slider: Id = msg_send![ns_slider, alloc];
        let slider: Id = msg_send![slider, initWithFrame: frame];

        let _: () = msg_send![slider, setMinValue: min];
        let _: () = msg_send![slider, setMaxValue: max];
        let _: () = msg_send![slider, setDoubleValue: value];
        // Fire action only on mouse-up to avoid spamming settings.json writes.
        let _: () = msg_send![slider, setContinuous: false];

        slider
    }
}
