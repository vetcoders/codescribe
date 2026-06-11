//! AppKit builder for the read-only Engine settings tab.

use super::*;

pub(super) unsafe fn build_engine_tab(
    action_handler: Id,
    frame: core_graphics::geometry::CGRect,
    config: &Config,
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
        let primary = crate::ui_helpers::color_label();
        let secondary = crate::ui_helpers::color_secondary_label();

        let title = create_label(LabelConfig {
            frame: CGRect::new(&CGPoint::new(pad, y), &CGSize::new(content_w, 24.0)),
            text: "Engine".to_string(),
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
            text: "Read-only runtime truth: STT, preview, keys, permissions, and daemon state."
                .to_string(),
            font_size: ui_tokens::MICRO_FONT_SIZE,
            text_color: secondary,
            ..Default::default()
        });
        add_subview(container, subtitle);
        y -= 16.0 + ui_tokens::SECTION_GAP;

        let runtime_header = create_label(LabelConfig {
            frame: CGRect::new(&CGPoint::new(pad, y), &CGSize::new(content_w, 18.0)),
            text: "Runtime Truth".to_string(),
            font_size: ui_tokens::SMALL_FONT_SIZE,
            bold: true,
            text_color: primary,
            ..Default::default()
        });
        add_subview(container, runtime_header);
        y -= 18.0 + gap;

        let preview_model = current_preview_timing_model();
        let runtime_rows = [
            (
                "Active STT:",
                if config.use_local_stt {
                    "Local final verdict"
                } else {
                    "Cloud final verdict"
                }
                .to_string(),
                secondary,
            ),
            (
                "Whisper language:",
                config.whisper_language.as_str().to_string(),
                secondary,
            ),
            (
                "Overlay preview:",
                if preview_model.overlay_enabled {
                    "ON".to_string()
                } else {
                    "OFF".to_string()
                },
                if preview_model.overlay_enabled {
                    ui_colors::status_granted()
                } else {
                    ui_colors::status_warning()
                },
            ),
            (
                "Preview cadence:",
                preview_timing_summary_text(preview_model),
                secondary,
            ),
            (
                "Formatting key:",
                key_status_text(formatting_key_is_set()).to_string(),
                key_status_color(formatting_key_is_set()),
            ),
            (
                "Assistive key:",
                key_status_text(keychain_key_is_set("LLM_ASSISTIVE_API_KEY")).to_string(),
                key_status_color(keychain_key_is_set("LLM_ASSISTIVE_API_KEY")),
            ),
        ];
        for (label, value, color) in runtime_rows {
            add_engine_metric_row(container, &mut y, pad, content_w, label, &value, color, gap);
        }

        y = add_tafla_header_separator(container, pad, y, content_w);
        y -= ui_tokens::SECTION_GAP;

        let matrix_header = create_label(LabelConfig {
            frame: CGRect::new(&CGPoint::new(pad, y), &CGSize::new(content_w, 18.0)),
            text: "Permission Matrix".to_string(),
            font_size: ui_tokens::SMALL_FONT_SIZE,
            bold: true,
            text_color: primary,
            ..Default::default()
        });
        add_subview(container, matrix_header);
        y -= 18.0 + gap;

        let mut diagnostics_permission_labels: [Option<usize>; 5] = [None; 5];
        for kind in PERMISSION_ORDER {
            let status = permission_status(kind);
            let name_label = create_label(LabelConfig {
                frame: CGRect::new(&CGPoint::new(pad, y), &CGSize::new(188.0, 20.0)),
                text: permission_row_label(kind).to_string(),
                font_size: ui_tokens::SMALL_FONT_SIZE,
                text_color: secondary,
                ..Default::default()
            });
            add_subview(container, name_label);

            let value_label = create_label(LabelConfig {
                frame: CGRect::new(&CGPoint::new(pad + 192.0, y), &CGSize::new(200.0, 20.0)),
                text: permission_status_text(status).to_string(),
                font_size: ui_tokens::SMALL_FONT_SIZE,
                text_color: permission_status_color(status),
                ..Default::default()
            });
            add_subview(container, value_label);
            diagnostics_permission_labels[kind.index()] = Some(value_label as usize);
            y -= 20.0 + gap;
        }
        state.diagnostics_permission_labels = diagnostics_permission_labels;

        y = add_tafla_header_separator(container, pad, y, content_w);
        y -= ui_tokens::SECTION_GAP;

        let conflicts_header = create_label(LabelConfig {
            frame: CGRect::new(&CGPoint::new(pad, y), &CGSize::new(content_w, 18.0)),
            text: "Hotkey Conflicts".to_string(),
            font_size: ui_tokens::SMALL_FONT_SIZE,
            bold: true,
            text_color: primary,
            ..Default::default()
        });
        add_subview(container, conflicts_header);
        y -= 18.0 + gap;

        let (conflict_text, has_conflict) = hotkey_conflict_status(config);
        let conflict_label = create_label(LabelConfig {
            frame: CGRect::new(&CGPoint::new(pad, y), &CGSize::new(content_w - 130.0, 28.0)),
            text: conflict_text,
            font_size: ui_tokens::MICRO_FONT_SIZE,
            text_color: if has_conflict {
                ui_colors::bubble_error_text()
            } else {
                secondary
            },
            ..Default::default()
        });
        add_subview(container, conflict_label);
        state.diagnostics_conflict_label = Some(conflict_label as usize);

        let conflict_details_button = create_button(
            CGRect::new(
                &CGPoint::new(pad + content_w - 120.0, y + 2.0),
                &CGSize::new(120.0, 24.0),
            ),
            "View conflicts",
            button_style::GLASS,
        );
        button_set_action(
            conflict_details_button,
            action_handler,
            sel!(onShowHotkeyConflicts:),
        );
        let _: () = msg_send![conflict_details_button, setEnabled: has_conflict];
        add_subview(container, conflict_details_button);
        state.diagnostics_conflict_details_button = Some(conflict_details_button as usize);
        y -= 28.0 + gap;

        let status_label = create_label(LabelConfig {
            frame: CGRect::new(&CGPoint::new(pad, y), &CGSize::new(content_w - 148.0, 16.0)),
            text: "Use Copy diagnostics to capture a full environment + permission report."
                .to_string(),
            font_size: ui_tokens::MICRO_FONT_SIZE,
            text_color: secondary,
            ..Default::default()
        });
        add_subview(container, status_label);
        state.diagnostics_status_label = Some(status_label as usize);

        let copy_diag_btn = create_button(
            CGRect::new(
                &CGPoint::new(pad + content_w - 138.0, y - 2.0),
                &CGSize::new(138.0, 24.0),
            ),
            "Copy diagnostics",
            button_style::GLASS,
        );
        button_set_action(copy_diag_btn, action_handler, sel!(onCopyDiagnostics:));
        add_subview(container, copy_diag_btn);
        y -= 24.0 + ui_tokens::SECTION_GAP;

        y = add_tafla_header_separator(container, pad, y, content_w);
        y -= ui_tokens::SECTION_GAP;

        let dashboard_header = create_label(LabelConfig {
            frame: CGRect::new(&CGPoint::new(pad, y), &CGSize::new(content_w, 18.0)),
            text: "Quality Daemon".to_string(),
            font_size: ui_tokens::SMALL_FONT_SIZE,
            bold: true,
            text_color: primary,
            ..Default::default()
        });
        add_subview(container, dashboard_header);
        y -= 18.0 + gap;

        let snapshot = crate::qube_lifecycle::dashboard_snapshot();
        let daemon_state = &snapshot.daemon_state;
        state.quality_available_label = Some(add_engine_metric_row(
            container,
            &mut y,
            pad,
            content_w,
            "Availability:",
            snapshot.availability_label(),
            if snapshot.available {
                ui_colors::status_granted()
            } else {
                ui_colors::status_warning()
            },
            gap,
        ));
        state.quality_pending_label = Some(add_engine_metric_row(
            container,
            &mut y,
            pad,
            content_w,
            "Pending:",
            &daemon_state.pending_mismatches.to_string(),
            if daemon_state.pending_mismatches > 0 {
                ui_colors::status_warning()
            } else {
                secondary
            },
            gap,
        ));
        state.quality_last_check_label = Some(add_engine_metric_row(
            container,
            &mut y,
            pad,
            content_w,
            "Last check:",
            &quality_last_check_text(&daemon_state.last_check),
            secondary,
            gap,
        ));
        state.qube_report_label = Some(add_engine_metric_row(
            container,
            &mut y,
            pad,
            content_w,
            "Latest report:",
            &qube_report_text(daemon_state),
            secondary,
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

fn add_engine_metric_row(
    container: Id,
    y: &mut f64,
    pad: f64,
    width: f64,
    label: &str,
    value: &str,
    value_color: Id,
    gap: f64,
) -> usize {
    use core_graphics::geometry::{CGPoint, CGRect, CGSize};
    let label_view = create_label(LabelConfig {
        frame: CGRect::new(&CGPoint::new(pad, *y), &CGSize::new(132.0, 18.0)),
        text: label.to_string(),
        font_size: ui_tokens::SMALL_FONT_SIZE,
        text_color: crate::ui_helpers::color_secondary_label(),
        ..Default::default()
    });
    // SAFETY: caller builds this row on the Settings AppKit main-thread path.
    unsafe {
        add_subview(container, label_view);
    }

    let value_view = create_label(LabelConfig {
        frame: CGRect::new(
            &CGPoint::new(pad + 136.0, *y),
            &CGSize::new((width - 136.0).max(120.0), 18.0),
        ),
        text: value.to_string(),
        font_size: ui_tokens::SMALL_FONT_SIZE,
        text_color: value_color,
        ..Default::default()
    });
    // SAFETY: caller builds this row on the Settings AppKit main-thread path.
    unsafe {
        add_subview(container, value_view);
    }
    *y -= 18.0 + gap;
    value_view as usize
}
