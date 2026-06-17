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
        let title_text = "Engine";
        let title_h =
            measured_label_height(title_text, content_w, ui_tokens::TITLE_FONT_SIZE, true)
                .max(24.0);
        let mut y = frame.size.height - (title_h + gap);
        let primary = crate::ui_helpers::color_label();
        let secondary = crate::ui_helpers::color_secondary_label();

        let title = create_label(LabelConfig {
            frame: CGRect::new(&CGPoint::new(pad, y), &CGSize::new(content_w, title_h)),
            text: title_text.to_string(),
            font_size: ui_tokens::TITLE_FONT_SIZE,
            bold: true,
            text_color: primary,
            ..Default::default()
        });
        configure_wrapping_label(title);
        add_subview(container, title);
        y -= title_h + gap;

        y = add_tafla_header_separator(container, pad, y, content_w);
        y -= gap;

        let subtitle_text =
            "Read-only runtime truth: STT, preview, keys, permissions, and daemon state.";
        let subtitle_h =
            measured_label_height(subtitle_text, content_w, ui_tokens::MICRO_FONT_SIZE, false);
        let subtitle = create_label(LabelConfig {
            frame: CGRect::new(&CGPoint::new(pad, y), &CGSize::new(content_w, subtitle_h)),
            text: subtitle_text.to_string(),
            font_size: ui_tokens::MICRO_FONT_SIZE,
            text_color: secondary,
            ..Default::default()
        });
        configure_wrapping_label(subtitle);
        add_subview(container, subtitle);
        y -= subtitle_h + ui_tokens::SECTION_GAP;

        let runtime_card_top =
            add_engine_section_header(container, &mut y, pad, content_w, "Runtime Truth");

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
            add_engine_metric_row(container, &mut y, pad, content_w, label, &value, color);
        }
        add_settings_group_card_dynamic(
            container,
            pad - 10.0,
            runtime_card_top,
            content_w + 20.0,
            y,
        );

        y = add_tafla_header_separator(container, pad, y, content_w);
        y -= ui_tokens::SECTION_GAP;

        let matrix_card_top =
            add_engine_section_header(container, &mut y, pad, content_w, "Permission Matrix");

        let mut diagnostics_permission_labels: [Option<usize>; 5] = [None; 5];
        for kind in PERMISSION_ORDER {
            let status = permission_status(kind);
            let name_text = permission_row_label(kind);
            let value_text = permission_status_text(status);
            let name_width = (content_w * 0.46).clamp(120.0, 188.0);
            let value_gap = 4.0;
            let value_x = pad + name_width + value_gap;
            let value_width = (content_w - name_width - value_gap).max(120.0);
            let row_h =
                measured_label_height(name_text, name_width, ui_tokens::SMALL_FONT_SIZE, false)
                    .max(measured_label_height(
                        value_text,
                        value_width,
                        ui_tokens::SMALL_FONT_SIZE,
                        false,
                    ))
                    .max(20.0);
            let name_label = create_label(LabelConfig {
                frame: CGRect::new(&CGPoint::new(pad, y), &CGSize::new(name_width, row_h)),
                text: name_text.to_string(),
                font_size: ui_tokens::SMALL_FONT_SIZE,
                text_color: secondary,
                ..Default::default()
            });
            configure_wrapping_label(name_label);
            add_subview(container, name_label);

            let value_label = create_label(LabelConfig {
                frame: CGRect::new(&CGPoint::new(value_x, y), &CGSize::new(value_width, row_h)),
                text: value_text.to_string(),
                font_size: ui_tokens::SMALL_FONT_SIZE,
                text_color: permission_status_color(status),
                ..Default::default()
            });
            configure_wrapping_label(value_label);
            add_subview(container, value_label);
            diagnostics_permission_labels[kind.index()] = Some(value_label as usize);
            y -= row_h + gap;
        }
        state.diagnostics_permission_labels = diagnostics_permission_labels;
        add_settings_group_card_dynamic(
            container,
            pad - 10.0,
            matrix_card_top,
            content_w + 20.0,
            y,
        );

        y = add_tafla_header_separator(container, pad, y, content_w);
        y -= ui_tokens::SECTION_GAP;

        let conflicts_card_top =
            add_engine_section_header(container, &mut y, pad, content_w, "Hotkey Conflicts");

        let (conflict_text, has_conflict) = hotkey_conflict_status(config);
        let conflict_text_width = (content_w - 130.0).max(80.0);
        let conflict_h = measured_label_height(
            &conflict_text,
            conflict_text_width,
            ui_tokens::MICRO_FONT_SIZE,
            false,
        )
        .max(28.0);
        let conflict_label = create_label(LabelConfig {
            frame: CGRect::new(
                &CGPoint::new(pad, y),
                &CGSize::new(conflict_text_width, conflict_h),
            ),
            text: conflict_text,
            font_size: ui_tokens::MICRO_FONT_SIZE,
            text_color: if has_conflict {
                ui_colors::bubble_error_text()
            } else {
                secondary
            },
            ..Default::default()
        });
        configure_wrapping_label(conflict_label);
        add_subview(container, conflict_label);
        state.diagnostics_conflict_label = Some(conflict_label as usize);

        let conflict_details_button = create_button(
            CGRect::new(
                &CGPoint::new(
                    pad + content_w - 120.0,
                    y + ((conflict_h - 24.0) / 2.0).max(0.0),
                ),
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
        y -= conflict_h + gap;

        let status_text = "Use Copy diagnostics to capture a full environment + permission report.";
        let status_text_width = (content_w - 148.0).max(80.0);
        let status_h = measured_label_height(
            status_text,
            status_text_width,
            ui_tokens::MICRO_FONT_SIZE,
            false,
        )
        .max(24.0);
        let status_label = create_label(LabelConfig {
            frame: CGRect::new(
                &CGPoint::new(pad, y),
                &CGSize::new(status_text_width, status_h),
            ),
            text: status_text.to_string(),
            font_size: ui_tokens::MICRO_FONT_SIZE,
            text_color: secondary,
            ..Default::default()
        });
        configure_wrapping_label(status_label);
        add_subview(container, status_label);
        state.diagnostics_status_label = Some(status_label as usize);

        let copy_diag_btn = create_button(
            CGRect::new(
                &CGPoint::new(
                    pad + content_w - 138.0,
                    y + ((status_h - 24.0) / 2.0).max(0.0),
                ),
                &CGSize::new(138.0, 24.0),
            ),
            "Copy diagnostics",
            button_style::GLASS,
        );
        button_set_action(copy_diag_btn, action_handler, sel!(onCopyDiagnostics:));
        add_subview(container, copy_diag_btn);
        let conflicts_content_bottom_y = y - status_h;
        add_settings_group_card_dynamic(
            container,
            pad - 10.0,
            conflicts_card_top,
            content_w + 20.0,
            conflicts_content_bottom_y,
        );
        y = conflicts_content_bottom_y - ui_tokens::SECTION_GAP;

        y = add_tafla_header_separator(container, pad, y, content_w);
        y -= ui_tokens::SECTION_GAP;

        let dashboard_card_top =
            add_engine_section_header(container, &mut y, pad, content_w, "Quality Daemon");

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
        ));
        state.quality_last_check_label = Some(add_engine_metric_row(
            container,
            &mut y,
            pad,
            content_w,
            "Last check:",
            &quality_last_check_text(&daemon_state.last_check),
            secondary,
        ));
        state.qube_report_label = Some(add_engine_metric_row(
            container,
            &mut y,
            pad,
            content_w,
            "Latest report:",
            &qube_report_text(daemon_state),
            secondary,
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
        // Both buttons share origin `y - 2.0`; close the daemon card below them.
        add_settings_group_card_dynamic(
            container,
            pad - 10.0,
            dashboard_card_top,
            content_w + 20.0,
            y - 2.0,
        );

        y -= 24.0 + ui_tokens::SECTION_GAP;
        y = add_tafla_header_separator(container, pad, y, content_w);
        y -= ui_tokens::SECTION_GAP;
        build_mcp_section(container, &mut y, pad, content_w);

        container
    }
}

/// Render the read-only "MCP Servers" runtime-truth section.
///
/// MCP servers are discovered when the Agent runtime initializes
/// (`agent::tools::mcp::register`). Discovery used to fail silently into
/// `tracing::warn!`, so the Settings UI showed *nothing* — indistinguishable
/// from "MCP doesn't exist" even when `~/.codescribe/mcp.json` was present and a
/// server simply failed to spawn. This section reads the cheap config probe plus
/// the cached runtime discovery report so the UI tells the truth: config path,
/// whether the file exists/parses, and each server's real state + failure
/// reason.
fn build_mcp_section(container: Id, y: &mut f64, pad: f64, content_w: f64) {
    use crate::agent::tools::mcp::McpRowTone;
    let secondary = crate::ui_helpers::color_secondary_label();
    let status = crate::agent::tools::mcp::probe_mcp_status();

    let card_top = add_engine_section_header(container, y, pad, content_w, "MCP Servers");

    add_engine_metric_row(
        container,
        y,
        pad,
        content_w,
        "Config:",
        &status.config_path_display,
        secondary,
    );

    for row in status.summary_rows() {
        let color = match row.tone {
            McpRowTone::Good => ui_colors::status_granted(),
            McpRowTone::Warn => ui_colors::status_warning(),
            McpRowTone::Bad => ui_colors::bubble_error_text(),
            McpRowTone::Neutral => secondary,
        };
        add_engine_metric_row(container, y, pad, content_w, &row.label, &row.value, color);
    }

    // SAFETY: Settings AppKit main-thread build path.
    unsafe {
        add_settings_group_card_dynamic(container, pad - 10.0, card_top, content_w + 20.0, *y);
    }
}

fn add_engine_section_header(container: Id, y: &mut f64, pad: f64, width: f64, title: &str) -> f64 {
    use core_graphics::geometry::{CGPoint, CGRect, CGSize};
    let header_h = measured_label_height(title, width, ui_tokens::SMALL_FONT_SIZE, true).max(18.0);
    let card_top = *y + header_h + 10.0;
    let header = create_label(LabelConfig {
        frame: CGRect::new(&CGPoint::new(pad, *y), &CGSize::new(width, header_h)),
        text: title.to_string(),
        font_size: ui_tokens::SMALL_FONT_SIZE,
        bold: true,
        text_color: crate::ui_helpers::color_label(),
        ..Default::default()
    });
    configure_wrapping_label(header);
    // SAFETY: caller builds this header on the Settings AppKit main-thread path.
    unsafe {
        add_subview(container, header);
    }
    *y -= header_h + ui_tokens::DENSITY_COMFORTABLE;
    card_top
}

fn add_engine_metric_row(
    container: Id,
    y: &mut f64,
    pad: f64,
    width: f64,
    label: &str,
    value: &str,
    value_color: Id,
) -> usize {
    use core_graphics::geometry::{CGPoint, CGRect, CGSize};
    let label_width = 132.0;
    let value_x = pad + 136.0;
    let value_width = (width - 136.0).max(120.0);
    let row_height = measured_label_height(label, label_width, ui_tokens::SMALL_FONT_SIZE, false)
        .max(measured_label_height(
            value,
            value_width,
            ui_tokens::SMALL_FONT_SIZE,
            false,
        ))
        .max(18.0);
    let label_view = create_label(LabelConfig {
        frame: CGRect::new(
            &CGPoint::new(pad, *y),
            &CGSize::new(label_width, row_height),
        ),
        text: label.to_string(),
        font_size: ui_tokens::SMALL_FONT_SIZE,
        text_color: crate::ui_helpers::color_secondary_label(),
        ..Default::default()
    });
    configure_wrapping_label(label_view);
    // SAFETY: caller builds this row on the Settings AppKit main-thread path.
    unsafe {
        add_subview(container, label_view);
    }

    let value_view = create_label(LabelConfig {
        frame: CGRect::new(
            &CGPoint::new(value_x, *y),
            &CGSize::new(value_width, row_height),
        ),
        text: value.to_string(),
        font_size: ui_tokens::SMALL_FONT_SIZE,
        text_color: value_color,
        ..Default::default()
    });
    configure_wrapping_label(value_view);
    // SAFETY: caller builds this row on the Settings AppKit main-thread path.
    unsafe {
        add_subview(container, value_view);
    }
    *y -= row_height + ui_tokens::DENSITY_COMFORTABLE;
    value_view as usize
}

fn measured_label_height(text: &str, width: f64, font_size: f64, bold: bool) -> f64 {
    use core_graphics::geometry::{CGRect, CGSize};

    unsafe {
        let ns_font = Class::get("NSFont").unwrap();
        let ns_dict = Class::get("NSDictionary").unwrap();
        let font: Id = if bold {
            msg_send![ns_font, boldSystemFontOfSize: font_size]
        } else {
            msg_send![ns_font, systemFontOfSize: font_size]
        };
        let font_key = ns_string("NSFont");
        let attrs: Id = msg_send![ns_dict, dictionaryWithObject: font forKey: font_key];
        let opts: u64 = 1 | 2; // NSStringDrawingUsesLineFragmentOrigin | NSStringDrawingUsesFontLeading
        let text_str = ns_string(text);
        let rect: CGRect = msg_send![
            text_str,
            boundingRectWithSize: CGSize::new(width.max(1.0), 10_000.0)
            options: opts
            attributes: attrs
        ];
        rect.size.height.ceil().max((font_size * 1.35).ceil())
    }
}

fn configure_wrapping_label(label: Id) {
    unsafe {
        let _: () = msg_send![label, setUsesSingleLineMode: false];
        let _: () = msg_send![label, setLineBreakMode: 0_isize];
        let cell: Id = msg_send![label, cell];
        if !cell.is_null() {
            let _: () = msg_send![cell, setWraps: true];
            let _: () = msg_send![cell, setLineBreakMode: 0_isize];
            let _: () = msg_send![cell, setScrollable: false];
        }
    }
}
