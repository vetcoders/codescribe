//! Modal NSAlert dialogs used by attachment and connector flows.

use super::*;

/// Show a modal text input dialog using NSAlert with an accessory NSTextField.
/// Returns the entered text, or None if the user cancelled.
pub fn show_text_input_dialog(title: &str, message: &str, placeholder: &str) -> Option<String> {
    unsafe {
        let ns_alert = Class::get("NSAlert").unwrap();
        let alert: Id = msg_send![ns_alert, new];
        let _: () = msg_send![alert, setMessageText: ns_string(title)];
        let _: () = msg_send![alert, setInformativeText: ns_string(message)];
        let _: () = msg_send![alert, addButtonWithTitle: ns_string("OK")];
        let _: () = msg_send![alert, addButtonWithTitle: ns_string("Cancel")];
        let _: () = msg_send![alert, setAlertStyle: 1_isize]; // NSAlertStyleInformational

        // Add a text field as accessory view.
        let ns_text_field = Class::get("NSTextField").unwrap();
        let field: Id = msg_send![ns_text_field, alloc];
        let field_frame = CGRect::new(
            &CGPoint::new(0.0, 0.0),
            &core_graphics::geometry::CGSize::new(300.0, 24.0),
        );
        let field: Id = msg_send![field, initWithFrame: field_frame];
        let _: () = msg_send![field, setPlaceholderString: ns_string(placeholder)];
        let _: () = msg_send![alert, setAccessoryView: field];

        // Make the text field first responder so it's focused.
        let window: Id = msg_send![alert, window];
        let _: () = msg_send![window, setInitialFirstResponder: field];

        // NSModalResponseOK (first button) = 1000
        let response: isize = msg_send![alert, runModal];
        if response != 1000 {
            return None;
        }
        let text: Id = msg_send![field, stringValue];
        if text.is_null() {
            return None;
        }
        let c_str: *const i8 = msg_send![text, UTF8String];
        if c_str.is_null() {
            return None;
        }
        let s = std::ffi::CStr::from_ptr(c_str)
            .to_string_lossy()
            .to_string();
        Some(s)
    }
}

/// Show a simple error alert.
pub fn show_error_alert(title: &str, message: &str) {
    unsafe {
        let ns_alert = Class::get("NSAlert").unwrap();
        let alert: Id = msg_send![ns_alert, new];
        let _: () = msg_send![alert, setMessageText: ns_string(title)];
        let _: () = msg_send![alert, setInformativeText: ns_string(message)];
        let _: () = msg_send![alert, setAlertStyle: 2_isize]; // NSAlertStyleCritical
        let _: () = msg_send![alert, runModal];
    }
}
