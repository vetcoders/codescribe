//! AppKit builder for the Voice Lab settings tab.

use super::*;

pub(super) unsafe fn build_quality_tab(
    action_handler: Id,
    frame: core_graphics::geometry::CGRect,
    _config: &Config,
    state: &mut SettingsWindowState,
) -> Id {
    use core_graphics::geometry::{CGPoint, CGRect, CGSize};
    // SAFETY: see module-level # Safety doc — main-thread AppKit / msg_send! access on retained `Id` pointers.
    unsafe {
        let ns_view = objc_class("NSView");
        let container: Id = msg_send![ns_view, alloc];
        let container: Id = msg_send![container, initWithFrame: frame];

        let pad = ui_tokens::EDGE_PADDING;
        let content_w = frame.size.width - pad * 2.0;
        let gap = ui_tokens::DENSITY_COMFORTABLE;
        let mut y = frame.size.height - (24.0 + gap);
        let mono_font_input = crate::ui_helpers::monospace_font(ui_tokens::BODY_FONT_SIZE);
        let primary = crate::ui_helpers::color_label();
        let secondary = crate::ui_helpers::color_secondary_label();
        let preview_model = current_preview_timing_model();

        let title = create_label(LabelConfig {
            frame: CGRect::new(&CGPoint::new(pad, y), &CGSize::new(content_w, 24.0)),
            text: "Voice Lab".to_string(),
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
            text: "Live preview cadence and final STT routing. Only knobs that materially improve UX live here."
                .to_string(),
            font_size: ui_tokens::MICRO_FONT_SIZE,
            text_color: secondary,
            ..Default::default()
        });
        add_subview(container, subtitle);
        y -= 16.0 + gap;

        add_settings_group_card(container, pad - 10.0, y + 28.0, content_w + 20.0, 142.0);
        let engine_header = create_label(LabelConfig {
            frame: CGRect::new(&CGPoint::new(pad, y), &CGSize::new(content_w, 18.0)),
            text: "Final Transcript Path".to_string(),
            font_size: ui_tokens::SMALL_FONT_SIZE,
            bold: true,
            text_color: primary,
            ..Default::default()
        });
        add_subview(container, engine_header);
        y -= 18.0 + gap;

        let provider_label = create_label(LabelConfig {
            frame: CGRect::new(&CGPoint::new(pad, y), &CGSize::new(92.0, 18.0)),
            text: "Commit with:".to_string(),
            font_size: ui_tokens::SMALL_FONT_SIZE,
            text_color: secondary,
            ..Default::default()
        });
        add_subview(container, provider_label);

        let ns_popup = objc_class("NSPopUpButton");
        let provider_popup: Id = msg_send![ns_popup, alloc];
        let provider_popup: Id = msg_send![provider_popup, initWithFrame:
            CGRect::new(&CGPoint::new(pad + 96.0, y - 2.0), &CGSize::new(220.0, 24.0))
            pullsDown: false
        ];
        let _: () = msg_send![provider_popup, addItemWithTitle: ns_string("Local final verdict")];
        let _: () = msg_send![provider_popup, addItemWithTitle: ns_string("Cloud final verdict")];
        let provider_index: isize = if _config.use_local_stt { 0 } else { 1 };
        let _: () = msg_send![provider_popup, selectItemAtIndex: provider_index];
        button_set_action(provider_popup, action_handler, sel!(onSttProviderChanged:));
        add_subview(container, provider_popup);
        y -= 24.0 + gap;

        let backend_note = if _config.use_local_stt {
            "Current mode: preview stays local, then the local verdict becomes the committed transcript. File-based final pass can strengthen that verdict or surface weak-truth warnings before paste/save."
        } else {
            "Current mode: preview stays local, then cloud STT becomes the committed verdict. If cloud does not return a reliable result, the app marks any surviving fallback as degraded and blocks silent auto-paste."
        };
        let provider_hint = create_label(LabelConfig {
            frame: CGRect::new(&CGPoint::new(pad, y), &CGSize::new(content_w, 16.0)),
            text: format!(
                "{backend_note} Endpoints ending with :stream use NDJSON and fit long buffered runs better."
            ),
            font_size: ui_tokens::MICRO_FONT_SIZE,
            text_color: secondary,
            ..Default::default()
        });
        add_subview(container, provider_hint);
        y -= 16.0 + gap;

        let stt_endpoint_val = std::env::var("STT_ENDPOINT").unwrap_or_default();
        let stt_endpoint_field = create_text_input(
            CGRect::new(
                &CGPoint::new(pad, y),
                &CGSize::new(content_w, SETTINGS_INPUT_HEIGHT),
            ),
            "Cloud final-verdict endpoint (multipart or ...:stream for NDJSON)",
            &stt_endpoint_val,
        );
        style_paper_input(stt_endpoint_field);
        let _: () = msg_send![stt_endpoint_field, setFont: mono_font_input];
        button_set_action(
            stt_endpoint_field,
            action_handler,
            sel!(onSttEndpointChanged:),
        );
        add_subview(container, stt_endpoint_field);
        y -= SETTINGS_INPUT_HEIGHT + gap;

        let stt_key_field = create_secure_text_input(
            CGRect::new(
                &CGPoint::new(pad, y),
                &CGSize::new(content_w, SETTINGS_INPUT_HEIGHT),
            ),
            "Cloud final-verdict API key (stored in Keychain; erase field to remove)",
        );
        style_paper_input(stt_key_field);
        let _: () = msg_send![stt_key_field, setFont: mono_font_input];
        button_set_action(stt_key_field, action_handler, sel!(onSttKeyChanged:));
        add_subview(container, stt_key_field);
        y -= SETTINGS_INPUT_HEIGHT + gap;

        y = add_tafla_header_separator(container, pad, y, content_w);
        y -= ui_tokens::SECTION_GAP;

        add_settings_group_card(container, pad - 10.0, y + 28.0, content_w + 20.0, 376.0);
        let preview_header = create_label(LabelConfig {
            frame: CGRect::new(&CGPoint::new(pad, y), &CGSize::new(content_w, 18.0)),
            text: "Preview Timing".to_string(),
            font_size: ui_tokens::SMALL_FONT_SIZE,
            bold: true,
            text_color: primary,
            ..Default::default()
        });
        add_subview(container, preview_header);
        y -= 18.0 + gap;

        let preview_hint = create_label(LabelConfig {
            frame: CGRect::new(&CGPoint::new(pad, y), &CGSize::new(content_w, 16.0)),
            text: "Live preview cadence. Buffer delay applies after the first visible partial."
                .to_string(),
            font_size: ui_tokens::MICRO_FONT_SIZE,
            text_color: secondary,
            ..Default::default()
        });
        add_subview(container, preview_hint);
        y -= 16.0 + gap;

        let preview_preset = detect_preset(preview_model);
        state.preview_advanced_expanded = matches!(preview_preset, PreviewTimingPreset::Custom);

        // Smooth anchor for C5b audit: 1038ms / 10.6 cps / 5 words / 8.0s.
        let preset_control_w = (content_w - 138.0).clamp(320.0, 420.0);
        let preset_segment = create_segmented_control(
            CGRect::new(
                &CGPoint::new(pad, y - 2.0),
                &CGSize::new(preset_control_w, 24.0),
            ),
            &PREVIEW_PRESET_LABELS,
        );
        let _: () = msg_send![
            preset_segment,
            setSelectedSegment: preview_preset.segment_index()
        ];
        button_set_action(
            preset_segment,
            action_handler,
            sel!(onPreviewPresetChanged:),
        );
        add_subview(container, preset_segment);
        state.preview_preset_segment = Some(preset_segment as usize);

        let env_override = preview_timing_has_env_override();
        let override_label = create_label(LabelConfig {
            frame: CGRect::new(
                &CGPoint::new(pad + preset_control_w + 10.0, y),
                &CGSize::new((content_w - preset_control_w - 10.0).max(96.0), 18.0),
            ),
            text: if env_override {
                "overridden by .env".to_string()
            } else {
                String::new()
            },
            font_size: ui_tokens::MICRO_FONT_SIZE,
            text_color: ui_colors::status_warning(),
            ..Default::default()
        });
        let _: () = msg_send![override_label, setHidden: !env_override];
        add_subview(container, override_label);
        state.preview_env_override_label = Some(override_label as usize);
        y -= 24.0 + gap;

        let advanced_button = create_button(
            CGRect::new(&CGPoint::new(pad, y - 2.0), &CGSize::new(132.0, 24.0)),
            if state.preview_advanced_expanded {
                "Hide advanced"
            } else {
                "Show advanced"
            },
            button_style::GLASS,
        );
        button_set_action(
            advanced_button,
            action_handler,
            sel!(onPreviewAdvancedToggled:),
        );
        add_subview(container, advanced_button);
        state.preview_advanced_button = Some(advanced_button as usize);
        y -= 24.0 + gap;

        let advanced_hidden = !state.preview_advanced_expanded;

        let buffer_row = add_slider_setting_row(
            container,
            action_handler,
            pad,
            &mut y,
            content_w,
            secondary,
            SliderSettingRowSpec {
                title: "Buffer delay:",
                value_text: &format!("{} ms", preview_model.buffer_delay_ms),
                min: 0.0,
                max: 1500.0,
                current: preview_model.buffer_delay_ms as f64,
                action: sel!(onPreviewBufferDelayChanged:),
                gap,
            },
        );
        for ptr in buffer_row.all_views() {
            let _: () = msg_send![ptr as Id, setHidden: advanced_hidden];
            state.preview_advanced_rows.push(ptr);
        }
        state.preview_buffer_delay_value_label = Some(buffer_row.value_label);
        state.preview_buffer_delay_slider = Some(buffer_row.slider);

        let typing_row = add_slider_setting_row(
            container,
            action_handler,
            pad,
            &mut y,
            content_w,
            secondary,
            SliderSettingRowSpec {
                title: "Typing speed:",
                value_text: &format!("{:.1} cps", preview_model.typing_cps),
                min: 5.0,
                max: 180.0,
                current: preview_model.typing_cps as f64,
                action: sel!(onPreviewTypingCpsChanged:),
                gap,
            },
        );
        for ptr in typing_row.all_views() {
            let _: () = msg_send![ptr as Id, setHidden: advanced_hidden];
            state.preview_advanced_rows.push(ptr);
        }
        state.preview_typing_cps_value_label = Some(typing_row.value_label);
        state.preview_typing_cps_slider = Some(typing_row.slider);

        let emit_row = add_slider_setting_row(
            container,
            action_handler,
            pad,
            &mut y,
            content_w,
            secondary,
            SliderSettingRowSpec {
                title: "Words per tick:",
                value_text: &format!("{} words", preview_model.emit_words_max),
                min: 1.0,
                max: 10.0,
                current: preview_model.emit_words_max as f64,
                action: sel!(onPreviewEmitWordsMaxChanged:),
                gap,
            },
        );
        for ptr in emit_row.all_views() {
            let _: () = msg_send![ptr as Id, setHidden: advanced_hidden];
            state.preview_advanced_rows.push(ptr);
        }
        state.preview_emit_words_max_value_label = Some(emit_row.value_label);
        state.preview_emit_words_max_slider = Some(emit_row.slider);

        let interim_row = add_slider_setting_row(
            container,
            action_handler,
            pad,
            &mut y,
            content_w,
            secondary,
            SliderSettingRowSpec {
                title: "Interim cadence:",
                value_text: &if (preview_model.effective_interim_sec
                    - preview_model.requested_interim_sec)
                    .abs()
                    > f32::EPSILON
                {
                    format!(
                        "{:.1} s -> {:.1} s effective",
                        preview_model.requested_interim_sec, preview_model.effective_interim_sec
                    )
                } else {
                    format!("{:.1} s", preview_model.effective_interim_sec)
                },
                min: 1.0,
                max: 12.0,
                current: preview_model.requested_interim_sec as f64,
                action: sel!(onPreviewInterimCadenceChanged:),
                gap,
            },
        );
        for ptr in interim_row.all_views() {
            let _: () = msg_send![ptr as Id, setHidden: advanced_hidden];
            state.preview_advanced_rows.push(ptr);
        }
        state.preview_interim_sec_value_label = Some(interim_row.value_label);
        state.preview_interim_sec_slider = Some(interim_row.slider);

        let preview_summary = create_label(LabelConfig {
            frame: CGRect::new(&CGPoint::new(pad, y), &CGSize::new(content_w, 16.0)),
            text: preview_timing_summary_text(preview_model),
            font_size: ui_tokens::MICRO_FONT_SIZE,
            text_color: secondary,
            ..Default::default()
        });
        add_subview(container, preview_summary);
        state.preview_timing_summary_label = Some(preview_summary as usize);
        y -= 16.0 + gap;

        let preview_box_height = 168.0;
        let (preview_scroll, preview_text_view) = create_scrollable_text_view(
            CGRect::new(
                &CGPoint::new(pad, y - preview_box_height),
                &CGSize::new(content_w, preview_box_height),
            ),
            false,
        );
        let _: () = msg_send![preview_text_view, setFont: mono_font_input];
        set_text_view_string(
            preview_text_view,
            &preview_timing_report_text(preview_model),
        );
        add_subview(container, preview_scroll);
        state.preview_timing_text_view = Some(preview_text_view as usize);
        y -= preview_box_height + gap;

        let routing_hint = create_label(LabelConfig {
            frame: CGRect::new(&CGPoint::new(pad, y), &CGSize::new(content_w, 16.0)),
            text: "Whisper language lives in Audio. AI formatting and slow-moving user toggles live in User."
                .to_string(),
            font_size: ui_tokens::MICRO_FONT_SIZE,
            text_color: secondary,
            ..Default::default()
        });
        add_subview(container, routing_hint);

        container
    }
}

// ============================================================================
// Diagnostics tab
// ============================================================================
