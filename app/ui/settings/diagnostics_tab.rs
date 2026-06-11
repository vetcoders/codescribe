//! AppKit builder for the Diagnostics settings tab.

use super::*;

pub(super) unsafe fn build_diagnostics_tab(
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
            text: "Diagnostics".to_string(),
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
            text: "Permissions, shortcut overlap, and one-click diagnostics copy.".to_string(),
            font_size: ui_tokens::MICRO_FONT_SIZE,
            text_color: secondary,
            ..Default::default()
        });
        add_subview(container, subtitle);
        y -= 16.0 + gap;

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
            let idx = kind.index();
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
            diagnostics_permission_labels[idx] = Some(value_label as usize);

            y -= 20.0 + gap;
        }
        state.diagnostics_permission_labels = diagnostics_permission_labels;

        let matrix_actions_y = y - 2.0;
        let refresh_matrix_btn = create_button(
            CGRect::new(
                &CGPoint::new(pad, matrix_actions_y),
                &CGSize::new(124.0, 24.0),
            ),
            "Refresh matrix",
            button_style::GLASS,
        );
        button_set_action(
            refresh_matrix_btn,
            action_handler,
            sel!(onDiagnosticsRefresh:),
        );
        add_subview(container, refresh_matrix_btn);

        let open_settings_btn = create_button(
            CGRect::new(
                &CGPoint::new(pad + 134.0, matrix_actions_y),
                &CGSize::new(154.0, 24.0),
            ),
            "Open System Settings",
            button_style::GLASS,
        );
        button_set_action(
            open_settings_btn,
            action_handler,
            sel!(onOpenSystemSettings:),
        );
        add_subview(container, open_settings_btn);

        let copy_diag_btn = create_button(
            CGRect::new(
                &CGPoint::new(pad + 298.0, matrix_actions_y),
                &CGSize::new(138.0, 24.0),
            ),
            "Copy diagnostics",
            button_style::GLASS,
        );
        button_set_action(copy_diag_btn, action_handler, sel!(onCopyDiagnostics:));
        add_subview(container, copy_diag_btn);
        y -= 24.0 + gap;

        let divider = create_label(LabelConfig {
            frame: CGRect::new(&CGPoint::new(pad, y), &CGSize::new(content_w, 1.0)),
            text: String::new(),
            background_color: Some(ui_colors::surface_border()),
            ..Default::default()
        });
        let _: () = msg_send![divider, setAlphaValue: 0.9f64];
        add_subview(container, divider);
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
            frame: CGRect::new(&CGPoint::new(pad, y), &CGSize::new(content_w, 16.0)),
            text: "Use Copy diagnostics to capture a full environment + permission report."
                .to_string(),
            font_size: ui_tokens::MICRO_FONT_SIZE,
            text_color: secondary,
            ..Default::default()
        });
        add_subview(container, status_label);
        state.diagnostics_status_label = Some(status_label as usize);

        container
    }
}

// ============================================================================
// Settings handlers
// ============================================================================
