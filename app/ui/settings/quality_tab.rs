//! AppKit builder for the Transcription quality settings tab.

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
        let field_w = content_w;
        let gap = ui_tokens::DENSITY_COMFORTABLE;
        let mut y = frame.size.height - (24.0 + gap);
        let mono_font_input = crate::ui_helpers::monospace_font(ui_tokens::BODY_FONT_SIZE);
        let primary = crate::ui_helpers::color_label();
        let secondary = crate::ui_helpers::color_secondary_label();
        let preview_model = current_preview_timing_model();

        let title = create_label(LabelConfig {
            frame: CGRect::new(&CGPoint::new(pad, y), &CGSize::new(content_w, 24.0)),
            text: "Transcription".to_string(),
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
            text: "Choose the backend for the committed transcript. Overlay preview stays local and provisional."
                .to_string(),
            font_size: ui_tokens::MICRO_FONT_SIZE,
            text_color: secondary,
            ..Default::default()
        });
        add_subview(container, subtitle);
        y -= 16.0 + gap;

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

        state.preview_buffer_delay_value_label = Some(add_slider_setting_row(
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
        ));
        state.preview_typing_cps_value_label = Some(add_slider_setting_row(
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
        ));
        state.preview_emit_words_max_value_label = Some(add_slider_setting_row(
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
        ));
        state.preview_interim_sec_value_label = Some(add_slider_setting_row(
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
        ));

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

        y = add_tafla_header_separator(container, pad, y, content_w);
        y -= ui_tokens::SECTION_GAP;

        let final_header = create_label(LabelConfig {
            frame: CGRect::new(&CGPoint::new(pad, y), &CGSize::new(content_w, 18.0)),
            text: "Final Transcript".to_string(),
            font_size: ui_tokens::SMALL_FONT_SIZE,
            bold: true,
            text_color: primary,
            ..Default::default()
        });
        add_subview(container, final_header);
        y -= 18.0 + gap;

        let ultra_on = std::env::var("CODESCRIBE_LOCAL_STT_FINAL_PASS")
            .map(|v| parse_env_bool(&v))
            .unwrap_or(false);
        let ultra_check = add_toggle_row(
            container,
            action_handler,
            pad,
            &mut y,
            field_w,
            secondary,
            ToggleRowSpec {
                title: "Local file-based final pass",
                checked: ultra_on,
                action: sel!(onUltraQualityToggled:),
                description: Some(
                    "Re-runs local Whisper on the saved audio after capture ends. Best lever when preview looks fine but the committed verdict should be upgraded, downgraded, or blocked before paste/save.",
                ),
                tag: None,
                gap,
            },
        );
        state.ultra_quality_checkbox = Some(ultra_check as usize);

        let _fmt_check = add_toggle_row(
            container,
            action_handler,
            pad,
            &mut y,
            field_w,
            secondary,
            ToggleRowSpec {
                title: "AI Formatting",
                checked: _config.ai_formatting_enabled,
                action: sel!(onFormattingToggled:),
                description: Some(
                    "Uses the formatting model to clean up the committed transcript. If formatting fails, the raw transcript is preserved and labeled as a formatting fallback.",
                ),
                tag: None,
                gap,
            },
        );

        let fmt_level_label = create_label(LabelConfig {
            frame: CGRect::new(&CGPoint::new(pad, y), &CGSize::new(120.0, 18.0)),
            text: "Formatting level:".to_string(),
            font_size: ui_tokens::SMALL_FONT_SIZE,
            text_color: secondary,
            ..Default::default()
        });
        add_subview(container, fmt_level_label);

        let ns_popup = objc_class("NSPopUpButton");
        let fmt_popup: Id = msg_send![ns_popup, alloc];
        let fmt_popup: Id = msg_send![fmt_popup, initWithFrame:
            CGRect::new(&CGPoint::new(pad + 124.0, y - 2.0), &CGSize::new(240.0, 24.0))
            pullsDown: false
        ];
        let _: () = msg_send![fmt_popup, addItemWithTitle: ns_string("Raw")];
        let _: () = msg_send![fmt_popup, addItemWithTitle: ns_string("Medium")];
        let _: () = msg_send![fmt_popup, addItemWithTitle: ns_string("Creative")];
        let current_level = std::env::var("FORMATTING_LEVEL").unwrap_or_default();
        let sel_idx: isize = match current_level.as_str() {
            "raw" => 0,
            "medium" => 1,
            "creative" => 2,
            _ => 1,
        };
        let _: () = msg_send![fmt_popup, selectItemAtIndex: sel_idx];
        button_set_action(fmt_popup, action_handler, sel!(onFormattingLevelChanged:));
        add_subview(container, fmt_popup);
        y -= 24.0 + gap;

        let routing_hint = create_label(LabelConfig {
            frame: CGRect::new(&CGPoint::new(pad, y), &CGSize::new(content_w, 16.0)),
            text: "Whisper language lives in Audio & Input. AI endpoints and prompts live in AI & Prompts."
                .to_string(),
            font_size: ui_tokens::MICRO_FONT_SIZE,
            text_color: secondary,
            ..Default::default()
        });
        add_subview(container, routing_hint);
        y -= 16.0 + gap;

        let divider = create_label(LabelConfig {
            frame: CGRect::new(&CGPoint::new(pad, y), &CGSize::new(field_w, 1.0)),
            text: String::new(),
            background_color: Some(ui_colors::surface_border()),
            ..Default::default()
        });
        let _: () = msg_send![divider, setAlphaValue: 0.9f64];
        add_subview(container, divider);
        y -= ui_tokens::SECTION_GAP;

        let automation_header = create_label(LabelConfig {
            frame: CGRect::new(&CGPoint::new(pad, y), &CGSize::new(content_w, 18.0)),
            text: "Quality Automation".to_string(),
            font_size: ui_tokens::SMALL_FONT_SIZE,
            bold: true,
            text_color: primary,
            ..Default::default()
        });
        add_subview(container, automation_header);
        y -= 18.0 + gap;

        let quality_on = UserSettings::load()
            .qube_daemon_autostart
            .unwrap_or_else(|| {
                std::env::var("QUBE_DAEMON_AUTOSTART")
                    .map(|v| parse_env_bool(&v))
                    .unwrap_or(false)
            });
        let quality_check = add_toggle_row(
            container,
            action_handler,
            pad,
            &mut y,
            field_w,
            secondary,
            ToggleRowSpec {
                title: "Start quality daemon automatically",
                checked: quality_on,
                action: sel!(onQubeDaemonToggled:),
                description: Some(
                    "Starts bundled `qube-daemon --daemon` immediately and on next CodeScribe launch when the binary is installed. \
                     Turning it off stops the daemon only when CodeScribe owns that process; externally managed launchd or shell runs remain untouched.",
                ),
                tag: None,
                gap,
            },
        );
        state.qube_daemon_checkbox = Some(quality_check as usize);

        let automation_hint = create_label(LabelConfig {
            frame: CGRect::new(&CGPoint::new(pad, y), &CGSize::new(content_w, 16.0)),
            text: "Use reports below to compare raw, postprocess, and final output when the runtime starts lying."
                .to_string(),
            font_size: ui_tokens::MICRO_FONT_SIZE,
            text_color: secondary,
            ..Default::default()
        });
        add_subview(container, automation_hint);
        y -= 16.0 + gap;

        let daemon_divider = create_label(LabelConfig {
            frame: CGRect::new(&CGPoint::new(pad, y), &CGSize::new(field_w, 1.0)),
            text: String::new(),
            background_color: Some(ui_colors::surface_border()),
            ..Default::default()
        });
        let _: () = msg_send![daemon_divider, setAlphaValue: 0.9f64];
        add_subview(container, daemon_divider);
        y -= ui_tokens::SECTION_GAP;

        let dashboard_header = create_label(LabelConfig {
            frame: CGRect::new(&CGPoint::new(pad, y), &CGSize::new(content_w, 18.0)),
            text: "Daemon State".to_string(),
            font_size: ui_tokens::SMALL_FONT_SIZE,
            bold: true,
            text_color: primary,
            ..Default::default()
        });
        add_subview(container, dashboard_header);
        y -= 18.0 + gap;

        let snapshot = crate::qube_lifecycle::dashboard_snapshot();
        let daemon_state = &snapshot.daemon_state;

        let add_metric_row = |container: Id,
                              y: &mut f64,
                              label: &str,
                              value: &str,
                              value_color: Id,
                              width: f64,
                              pad: f64,
                              gap: f64|
         -> usize {
            let label_view = create_label(LabelConfig {
                frame: CGRect::new(&CGPoint::new(pad, *y), &CGSize::new(124.0, 18.0)),
                text: label.to_string(),
                font_size: ui_tokens::SMALL_FONT_SIZE,
                text_color: crate::ui_helpers::color_secondary_label(),
                ..Default::default()
            });
            add_subview(container, label_view);

            let value_view = create_label(LabelConfig {
                frame: CGRect::new(
                    &CGPoint::new(pad + 126.0, *y),
                    &CGSize::new((width - 126.0).max(120.0), 18.0),
                ),
                text: value.to_string(),
                font_size: ui_tokens::SMALL_FONT_SIZE,
                text_color: value_color,
                ..Default::default()
            });
            add_subview(container, value_view);
            *y -= 18.0 + gap;
            value_view as usize
        };

        let available_text = snapshot.availability_label();
        let available_color = if snapshot.available {
            ui_colors::status_granted()
        } else {
            ui_colors::status_warning()
        };
        state.quality_available_label = Some(add_metric_row(
            container,
            &mut y,
            "Availability:",
            available_text,
            available_color,
            content_w,
            pad,
            gap,
        ));
        state.quality_pending_label = Some(add_metric_row(
            container,
            &mut y,
            "Pending:",
            &daemon_state.pending_mismatches.to_string(),
            if daemon_state.pending_mismatches > 0 {
                ui_colors::status_warning()
            } else {
                secondary
            },
            content_w,
            pad,
            gap,
        ));
        state.quality_last_check_label = Some(add_metric_row(
            container,
            &mut y,
            "Last check:",
            &quality_last_check_text(&daemon_state.last_check),
            secondary,
            content_w,
            pad,
            gap,
        ));
        state.qube_report_label = Some(add_metric_row(
            container,
            &mut y,
            "Latest report:",
            &qube_report_text(daemon_state),
            secondary,
            content_w,
            pad,
            gap,
        ));

        let refresh_btn = create_button(
            CGRect::new(&CGPoint::new(pad, y - 2.0), &CGSize::new(116.0, 24.0)),
            "Refresh status",
            button_style::GLASS,
        );
        button_set_action(refresh_btn, action_handler, sel!(onQualityRefresh:));
        add_subview(container, refresh_btn);

        let open_report_btn = create_button(
            CGRect::new(
                &CGPoint::new(pad + 126.0, y - 2.0),
                &CGSize::new(128.0, 24.0),
            ),
            "Open report",
            button_style::GLASS,
        );
        button_set_action(open_report_btn, action_handler, sel!(onOpenQualityReport:));
        let _: () = msg_send![open_report_btn, setEnabled: qube_report_exists(daemon_state)];
        add_subview(container, open_report_btn);
        state.quality_open_report_button = Some(open_report_btn as usize);

        container
    }
}

// ============================================================================
// Diagnostics tab
// ============================================================================
