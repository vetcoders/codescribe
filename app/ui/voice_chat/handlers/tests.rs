//! Unit tests for overlay handler classes and zoom canonicalization.

use super::*;

fn assert_selector_registered(class: *const Class, selector: Sel, label: &str) {
    // SAFETY: `overlay_window_class` registers a valid Objective-C class, and this
    // only asks the runtime whether instances handle the selector.
    let responds: bool = unsafe { msg_send![class, instancesRespondToSelector: selector] };
    assert!(
        responds,
        "VoiceChatOverlayWindow missing selector `{label}`"
    );
}

#[test]
fn test_canonical_zoom_level_rounds_and_clamps() {
    assert!((canonical_zoom_level(1.0) - 1.0).abs() < f64::EPSILON);
    assert!((canonical_zoom_level(1.129) - 1.13).abs() < 0.0001);
    assert!((canonical_zoom_level(0.2) - 0.75).abs() < 0.0001);
    assert!((canonical_zoom_level(2.8) - 2.0).abs() < 0.0001);
}

#[test]
fn overlay_window_subclass_keeps_floating_input_keyable() {
    assert!(overlay_window_allows_key_input());
    assert!(overlay_window_allows_main_status());

    let class = overlay_window_class();
    assert!(
        !class.is_null(),
        "VoiceChatOverlayWindow class should be registered"
    );

    assert_selector_registered(class, sel!(canBecomeKeyWindow), "canBecomeKeyWindow");
    assert_selector_registered(class, sel!(canBecomeMainWindow), "canBecomeMainWindow");
}

#[test]
fn agent_input_text_view_overrides_paste() {
    let class = agent_input_text_view_class();
    assert!(
        !class.is_null(),
        "VoiceChatAgentInputTextView class should be registered"
    );

    assert_selector_registered(class, sel!(paste:), "paste:");
}

#[test]
fn paste_disposition_matches_attachment_policy() {
    use PasteDisposition::{Attachment, TextPaste};

    let cases = [
        (true, false, false, Attachment),
        (true, true, false, Attachment),
        (true, true, true, Attachment),
        (false, true, false, Attachment),
        (false, false, false, TextPaste),
        (false, false, true, TextPaste),
        (false, true, true, TextPaste),
    ];

    for (has_files, has_image, has_text, expected) in cases {
        assert_eq!(
            paste_disposition(has_files, has_image, has_text),
            expected,
            "has_files={has_files}, has_image={has_image}, has_text={has_text}"
        );
    }
}
