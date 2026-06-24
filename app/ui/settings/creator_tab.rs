//! AppKit builder for the Creator settings launchpad tab.

use super::*;

pub(super) unsafe fn build_creator_tab(
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
        let primary = crate::ui_helpers::color_label();
        let secondary = crate::ui_helpers::color_secondary_label();

        let title = create_label(LabelConfig {
            frame: CGRect::new(&CGPoint::new(pad, y), &CGSize::new(content_w, 24.0)),
            text: "Creator".to_string(),
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
            text: "First-run truth, permissions, and launchpads into the working surfaces."
                .to_string(),
            font_size: ui_tokens::MICRO_FONT_SIZE,
            text_color: secondary,
            ..Default::default()
        });
        add_subview(container, subtitle);
        y -= 16.0 + ui_tokens::SECTION_GAP;

        add_settings_group_card(container, pad - 10.0, y + 28.0, content_w + 20.0, 218.0);
        let checklist_header = create_label(LabelConfig {
            frame: CGRect::new(&CGPoint::new(pad, y), &CGSize::new(content_w, 18.0)),
            text: "Permission Checklist".to_string(),
            font_size: ui_tokens::SMALL_FONT_SIZE,
            bold: true,
            text_color: primary,
            ..Default::default()
        });
        add_subview(container, checklist_header);
        y -= 18.0 + gap;

        let mut permission_labels: [Option<usize>; 5] = [None; 5];
        let mut permission_action_buttons: [Option<usize>; 5] = [None; 5];
        for (tag, kind) in PERMISSION_ORDER.iter().copied().enumerate() {
            let status = permission_status(kind);
            let granted = status == PermissionStatus::Granted;
            let label = create_label(LabelConfig {
                frame: CGRect::new(&CGPoint::new(pad, y), &CGSize::new(content_w - 134.0, 20.0)),
                text: format!(
                    "{} {}",
                    if granted { "OK" } else { "TODO" },
                    permission_row_label(kind)
                ),
                font_size: ui_tokens::SMALL_FONT_SIZE,
                text_color: permission_color(granted),
                ..Default::default()
            });
            add_subview(container, label);
            permission_labels[kind.index()] = Some(label as usize);

            let action_title = permission_action_title(kind, status, false).unwrap_or("Granted");
            let action_button = create_button(
                CGRect::new(
                    &CGPoint::new(pad + content_w - 120.0, y - 2.0),
                    &CGSize::new(120.0, 24.0),
                ),
                action_title,
                button_style::GLASS,
            );
            let _: () = msg_send![action_button, setTag: tag as isize];
            let _: () = msg_send![action_button, setHidden: granted];
            button_set_action(action_button, action_handler, sel!(onPermissionAction:));
            add_subview(container, action_button);
            permission_action_buttons[kind.index()] = Some(action_button as usize);

            y -= 24.0 + gap;
        }
        state.permission_labels = permission_labels;
        state.permission_action_buttons = permission_action_buttons;

        let refresh_btn = create_button(
            CGRect::new(&CGPoint::new(pad, y - 2.0), &CGSize::new(138.0, 24.0)),
            "Refresh permissions",
            button_style::GLASS,
        );
        button_set_action(refresh_btn, action_handler, sel!(onRefreshPermissions:));
        add_subview(container, refresh_btn);

        let open_settings_btn = create_button(
            CGRect::new(
                &CGPoint::new(pad + 148.0, y - 2.0),
                &CGSize::new(158.0, 24.0),
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
        y -= 24.0 + ui_tokens::SECTION_GAP;

        y = add_tafla_header_separator(container, pad, y, content_w);
        y -= ui_tokens::SECTION_GAP;

        add_settings_group_card(container, pad - 10.0, y + 28.0, content_w + 20.0, 178.0);
        let quick_header = create_label(LabelConfig {
            frame: CGRect::new(&CGPoint::new(pad, y), &CGSize::new(content_w, 18.0)),
            text: "Quick Start".to_string(),
            font_size: ui_tokens::SMALL_FONT_SIZE,
            bold: true,
            text_color: primary,
            ..Default::default()
        });
        add_subview(container, quick_header);
        y -= 18.0 + gap;

        let quick_specs = [
            (
                "Test microphone",
                "Starts a short local recording check.",
                "Test mic",
                sel!(onTestMic:),
                STEP_TEST_MIC,
            ),
            (
                "Open agent overlay",
                "Shows the chat overlay and selects Agent.",
                "Open overlay",
                sel!(onShowOverlay:),
                STEP_SHOW_OVERLAY,
            ),
            (
                "Tune shortcuts",
                "Jump to Keys when the permission checklist is green.",
                "Open Keys",
                sel!(onTabKeys:),
                STEP_PRESS_HOTKEY,
            ),
        ];

        for (title, description, button_title, action, step_idx) in quick_specs {
            let title_label = create_label(LabelConfig {
                frame: CGRect::new(&CGPoint::new(pad, y), &CGSize::new(content_w - 154.0, 18.0)),
                text: title.to_string(),
                font_size: ui_tokens::SMALL_FONT_SIZE,
                text_color: primary,
                ..Default::default()
            });
            add_subview(container, title_label);

            let action_button = create_button(
                CGRect::new(
                    &CGPoint::new(pad + content_w - 140.0, y - 2.0),
                    &CGSize::new(140.0, 24.0),
                ),
                button_title,
                button_style::GLASS,
            );
            button_set_action(action_button, action_handler, action);
            add_subview(container, action_button);

            y -= 18.0;
            let desc_label = create_label(LabelConfig {
                frame: CGRect::new(&CGPoint::new(pad, y), &CGSize::new(content_w - 154.0, 16.0)),
                text: description.to_string(),
                font_size: ui_tokens::MICRO_FONT_SIZE,
                text_color: secondary,
                ..Default::default()
            });
            add_subview(container, desc_label);

            let status_label = create_label(LabelConfig {
                frame: CGRect::new(
                    &CGPoint::new(pad + content_w - 140.0, y),
                    &CGSize::new(140.0, 16.0),
                ),
                text: "ready".to_string(),
                font_size: ui_tokens::MICRO_FONT_SIZE,
                text_color: secondary,
                ..Default::default()
            });
            add_subview(container, status_label);
            state.step_labels[step_idx] = Some(status_label as usize);

            y -= 16.0 + gap;
        }

        y = add_tafla_header_separator(container, pad, y, content_w);
        y -= ui_tokens::SECTION_GAP;

        add_settings_group_card(container, pad - 10.0, y + 28.0, content_w + 20.0, 94.0);
        let launch_header = create_label(LabelConfig {
            frame: CGRect::new(&CGPoint::new(pad, y), &CGSize::new(content_w, 18.0)),
            text: "Launchpads".to_string(),
            font_size: ui_tokens::SMALL_FONT_SIZE,
            bold: true,
            text_color: primary,
            ..Default::default()
        });
        add_subview(container, launch_header);
        y -= 18.0 + gap;

        let button_w = ((content_w - 16.0) / 3.0).max(120.0);
        let launchpads = [
            ("Keys", sel!(onTabKeys:)),
            ("Audio", sel!(onTabAudio:)),
            ("Voice Lab", sel!(onTabVoiceLab:)),
            ("Engine", sel!(onTabEngine:)),
            ("User", sel!(onTabUser:)),
        ];
        for (idx, (title, action)) in launchpads.iter().enumerate() {
            let col = idx % 3;
            let row = idx / 3;
            let x = pad + (button_w + 8.0) * col as f64;
            let button_y = y - (32.0 * row as f64);
            let button = create_button(
                CGRect::new(
                    &CGPoint::new(x, button_y - 2.0),
                    &CGSize::new(button_w, 24.0),
                ),
                title,
                button_style::GLASS,
            );
            button_set_action(button, action_handler, *action);
            add_subview(container, button);
        }

        container
    }
}
