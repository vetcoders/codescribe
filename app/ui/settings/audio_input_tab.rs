//! AppKit builder for the Audio Input settings tab.

use super::*;

pub(super) unsafe fn build_audio_input_tab(
    action_handler: Id,
    frame: core_graphics::geometry::CGRect,
    config: &Config,
) -> Id {
    use core_graphics::geometry::{CGPoint, CGRect, CGSize};
    // SAFETY: see module-level # Safety doc — main-thread AppKit / msg_send! access on retained `Id` pointers.
    unsafe {
        let ns_view = objc_class("NSView");
        let ns_popup = objc_class("NSPopUpButton");

        let container: Id = msg_send![ns_view, alloc];
        let container: Id = msg_send![container, initWithFrame: frame];

        let pad = ui_tokens::EDGE_PADDING;
        let content_w = frame.size.width - pad * 2.0;
        let gap = ui_tokens::DENSITY_COMFORTABLE;
        let mut y = frame.size.height - (24.0 + gap);
        let primary = crate::ui_helpers::color_label();
        let secondary = crate::ui_helpers::color_secondary_label();

        let title = create_label(LabelConfig {
            frame: CGRect::new(&CGPoint::new(pad, y), &CGSize::new(content_w, 24.0)),
            text: "Audio".to_string(),
            font_size: ui_tokens::TITLE_FONT_SIZE,
            bold: true,
            text_color: primary,
            ..Default::default()
        });
        add_subview(container, title);
        y -= 24.0 + gap;

        y = add_tafla_header_separator(container, pad, y, content_w);
        y -= gap;

        let subtitle = create_label(LabelConfig {
            frame: CGRect::new(&CGPoint::new(pad, y), &CGSize::new(content_w, 16.0)),
            text: "Speech capture defaults, recorder feedback, and simple input toggles."
                .to_string(),
            font_size: ui_tokens::MICRO_FONT_SIZE,
            text_color: secondary,
            ..Default::default()
        });
        add_subview(container, subtitle);
        y -= 16.0 + gap;

        add_settings_group_card(container, pad - 10.0, y + 28.0, content_w + 20.0, 284.0);
        let capture_header = create_label(LabelConfig {
            frame: CGRect::new(&CGPoint::new(pad, y), &CGSize::new(content_w, 18.0)),
            text: "Capture Defaults".to_string(),
            font_size: ui_tokens::SMALL_FONT_SIZE,
            bold: true,
            text_color: primary,
            ..Default::default()
        });
        add_subview(container, capture_header);
        y -= 18.0 + gap;

        let capture_hint = create_label(LabelConfig {
            frame: CGRect::new(&CGPoint::new(pad, y), &CGSize::new(content_w, 16.0)),
            text: "Language, capture feedback, overlay visibility, and agent send behavior."
                .to_string(),
            font_size: ui_tokens::MICRO_FONT_SIZE,
            text_color: secondary,
            ..Default::default()
        });
        add_subview(container, capture_hint);
        y -= 16.0 + gap;

        // Language dropdown
        let lang_label = create_label(LabelConfig {
            frame: CGRect::new(&CGPoint::new(pad, y), &CGSize::new(130.0, 18.0)),
            text: "Whisper language:".to_string(),
            font_size: ui_tokens::SMALL_FONT_SIZE,
            text_color: secondary,
            ..Default::default()
        });
        add_subview(container, lang_label);

        let lang_popup: Id = msg_send![ns_popup, alloc];
        let lang_popup: Id = msg_send![lang_popup, initWithFrame:
            CGRect::new(&CGPoint::new(pad + 134.0, y - 2.0), &CGSize::new(180.0, 24.0))
            pullsDown: false
        ];
        let _: () = msg_send![lang_popup, addItemWithTitle: ns_string("Polish (pl)")];
        let _: () = msg_send![lang_popup, addItemWithTitle: ns_string("English (en)")];
        let lang_idx: isize = match config.whisper_language.as_str() {
            "pl" => 0,
            "en" => 1,
            _ => 0,
        };
        let _: () = msg_send![lang_popup, selectItemAtIndex: lang_idx];
        button_set_action(lang_popup, action_handler, sel!(onLanguageChanged:));
        add_subview(container, lang_popup);
        y -= 24.0 + gap;

        // Beep on start toggle
        let _beep_check = add_toggle_row(
            container,
            action_handler,
            pad,
            &mut y,
            content_w,
            secondary,
            ToggleRowSpec {
                title: "Beep on recording start",
                checked: config.beep_on_start,
                action: sel!(onBeepToggled:),
                description: None,
                tag: None,
                gap,
            },
        );
        // Agent: Enter to send toggle
        let _enter_check = add_toggle_row(
            container,
            action_handler,
            pad,
            &mut y,
            content_w,
            secondary,
            ToggleRowSpec {
                title: "Enter to send (⌘⏎ for newline)",
                checked: config.agent_enter_sends,
                action: sel!(onEnterSendToggled:),
                description: None,
                tag: None,
                gap,
            },
        );
        let _overlay_check = add_toggle_row(
            container,
            action_handler,
            pad,
            &mut y,
            content_w,
            secondary,
            ToggleRowSpec {
                title: "Transcription overlay",
                checked: config.transcription_overlay_enabled,
                action: sel!(onTranscriptionOverlayToggled:),
                description: Some(
                    "On: live floating preview with fast partials. Off: no overlay and buffered partials for lower local load.",
                ),
                tag: None,
                gap,
            },
        );
        // Sound volume slider
        let vol_label = create_label(LabelConfig {
            frame: CGRect::new(&CGPoint::new(pad, y), &CGSize::new(120.0, 20.0)),
            text: "Sound volume:".to_string(),
            font_size: ui_tokens::SMALL_FONT_SIZE,
            text_color: secondary,
            ..Default::default()
        });
        add_subview(container, vol_label);

        let vol_slider = create_slider(
            CGRect::new(
                &CGPoint::new(pad + 124.0, y),
                &CGSize::new(content_w - 124.0, 20.0),
            ),
            0.0,
            1.0,
            config.sound_volume as f64,
        );
        button_set_action(vol_slider, action_handler, sel!(onVolumeChanged:));
        add_subview(container, vol_slider);

        container
    } // unsafe
}

// ============================================================================
// Transcription tab
// ============================================================================
