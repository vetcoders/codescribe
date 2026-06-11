//! AppKit builder for the Modes & Shortcuts settings tab.

use super::*;

pub(super) unsafe fn build_modes_shortcuts_tab(
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
            text: "Modes & Shortcuts".to_string(),
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
            text:
                "Mode-first shortcut model. Each mode has one binding you can customize or disable."
                    .to_string(),
            font_size: ui_tokens::MICRO_FONT_SIZE,
            text_color: secondary,
            ..Default::default()
        });
        add_subview(container, subtitle);
        y -= 16.0 + gap;

        let usage_hint = create_label(LabelConfig {
            frame: CGRect::new(&CGPoint::new(pad, y), &CGSize::new(content_w, 28.0)),
            text: "Hold records while pressed. Double-tap records hands-free; repeat the gesture to stop.".to_string(),
            font_size: ui_tokens::MICRO_FONT_SIZE,
            text_color: secondary,
            ..Default::default()
        });
        add_subview(container, usage_hint);
        y -= 28.0 + gap;

        let mode_specs = [
            (WorkMode::Dictation, MODE_DICTATION_TAG),
            (WorkMode::Formatting, MODE_FORMATTING_TAG),
            (WorkMode::Assistive, MODE_ASSISTIVE_TAG),
        ];
        let settings_snapshot = UserSettings::load();
        let mut mode_binding_labels: [Option<usize>; 3] = [None; 3];
        for (mode, tag) in mode_specs {
            let change_button_w = 96.0;
            let disable_button_w = 72.0;
            let button_gap = 8.0;
            let change_x = pad + content_w - change_button_w;
            let disable_x = change_x - button_gap - disable_button_w;
            let binding_right_x = disable_x - 8.0;

            let row_title = create_label(LabelConfig {
                frame: CGRect::new(&CGPoint::new(pad, y), &CGSize::new(126.0, 20.0)),
                text: format!("{}:", mode.label()),
                font_size: ui_tokens::SMALL_FONT_SIZE,
                text_color: secondary,
                ..Default::default()
            });
            add_subview(container, row_title);

            let binding_x = pad + 128.0;
            let binding_label = create_label(LabelConfig {
                frame: CGRect::new(
                    &CGPoint::new(binding_x, y),
                    &CGSize::new((binding_right_x - binding_x).max(140.0), 20.0),
                ),
                text: settings_snapshot.mode_binding_for(mode).label().to_string(),
                font_size: ui_tokens::SMALL_FONT_SIZE,
                text_color: secondary,
                ..Default::default()
            });
            add_subview(container, binding_label);
            mode_binding_labels[mode_label_slot(mode)] = Some(binding_label as usize);

            let disable_button = create_button(
                CGRect::new(
                    &CGPoint::new(disable_x, y - 2.0),
                    &CGSize::new(disable_button_w, 24.0),
                ),
                "Disable",
                button_style::GLASS,
            );
            let _: () = msg_send![disable_button, setTag: tag + MODE_DISABLE_TAG_OFFSET];
            button_set_action(disable_button, action_handler, sel!(onModeBindingChange:));
            add_subview(container, disable_button);

            let change_button = create_button(
                CGRect::new(
                    &CGPoint::new(change_x, y - 2.0),
                    &CGSize::new(change_button_w, 24.0),
                ),
                "\u{2328} Customize",
                button_style::GLASS,
            );
            let _: () = msg_send![change_button, setTag: tag];
            button_set_action(change_button, action_handler, sel!(onModeBindingChange:));
            add_subview(container, change_button);

            y -= 24.0;

            let mode_hint = create_label(LabelConfig {
                frame: CGRect::new(&CGPoint::new(pad, y), &CGSize::new(content_w, 14.0)),
                text: format!(
                    "{} {} {}",
                    mode.description(),
                    if mode.defaults_to_auto_paste() {
                        "Auto-paste: ON."
                    } else {
                        "Auto-paste: OFF."
                    },
                    if mode.forces_ai() {
                        "AI required."
                    } else {
                        "AI optional."
                    }
                ),
                font_size: ui_tokens::MICRO_FONT_SIZE,
                text_color: secondary,
                ..Default::default()
            });
            add_subview(container, mode_hint);
            y -= 14.0 + gap;

            if mode == WorkMode::Assistive {
                let selection_hint = create_label(LabelConfig {
                    frame: CGRect::new(&CGPoint::new(pad, y), &CGSize::new(content_w, 14.0)),
                    text: "Tip: Select text in the frontmost app before triggering Assistive to operate on the selection.".to_string(),
                    font_size: ui_tokens::MICRO_FONT_SIZE,
                    text_color: secondary,
                    ..Default::default()
                });
                add_subview(container, selection_hint);
                y -= 14.0 + gap;
            }
        }

        let recorder_hint = create_label(LabelConfig {
            frame: CGRect::new(&CGPoint::new(pad, y), &CGSize::new(content_w, 16.0)),
            text: "Shortcut recorder: click [⌨ Customize], press Fn/Ctrl/Option. Esc cancels."
                .to_string(),
            font_size: ui_tokens::MICRO_FONT_SIZE,
            text_color: secondary,
            ..Default::default()
        });
        add_subview(container, recorder_hint);
        y -= 16.0 + gap;

        if let Some(fn_note) = shortcut_registry::fn_tap_intercept_note(&settings_snapshot) {
            let fn_note_label = create_label(LabelConfig {
                frame: CGRect::new(&CGPoint::new(pad, y), &CGSize::new(content_w, 28.0)),
                text: fn_note.to_string(),
                font_size: ui_tokens::MICRO_FONT_SIZE,
                text_color: secondary,
                ..Default::default()
            });
            add_subview(container, fn_note_label);
            y -= 28.0 + gap;
        }

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
        y -= 28.0 + gap;

        let config_divider = create_label(LabelConfig {
            frame: CGRect::new(&CGPoint::new(pad, y), &CGSize::new(content_w, 1.0)),
            text: String::new(),
            background_color: Some(ui_colors::surface_border()),
            ..Default::default()
        });
        let _: () = msg_send![config_divider, setAlphaValue: 0.9f64];
        add_subview(container, config_divider);
        y -= gap;

        let api_hint = create_label(LabelConfig {
            frame: CGRect::new(&CGPoint::new(pad, y), &CGSize::new(content_w, 16.0)),
            text: "Tighten the shortcut feel here. These controls affect trigger responsiveness, not transcript quality."
                .to_string(),
            font_size: ui_tokens::MICRO_FONT_SIZE,
            text_color: secondary,
            ..Default::default()
        });
        add_subview(container, api_hint);
        y -= 16.0 + gap;

        let timing_divider = create_label(LabelConfig {
            frame: CGRect::new(&CGPoint::new(pad, y), &CGSize::new(content_w, 1.0)),
            text: String::new(),
            background_color: Some(ui_colors::surface_border()),
            ..Default::default()
        });
        let _: () = msg_send![timing_divider, setAlphaValue: 0.9f64];
        add_subview(container, timing_divider);
        y -= ui_tokens::SECTION_GAP;

        let timing_header = create_label(LabelConfig {
            frame: CGRect::new(&CGPoint::new(pad, y), &CGSize::new(content_w, 18.0)),
            text: "Trigger Timing".to_string(),
            font_size: ui_tokens::SMALL_FONT_SIZE,
            bold: true,
            text_color: primary,
            ..Default::default()
        });
        add_subview(container, timing_header);
        y -= 18.0 + gap;

        let delay_label = create_label(LabelConfig {
            frame: CGRect::new(&CGPoint::new(pad, y), &CGSize::new(120.0, 20.0)),
            text: "Hold delay (ms):".to_string(),
            font_size: ui_tokens::SMALL_FONT_SIZE,
            text_color: secondary,
            ..Default::default()
        });
        add_subview(container, delay_label);

        let delay_ms = config.hold_start_delay_ms as f64;
        let value_w = 60.0;
        let value_gap = 8.0;
        let slider_w = (content_w - 124.0 - value_gap - value_w).max(120.0);
        let delay_slider = create_slider(
            CGRect::new(&CGPoint::new(pad + 124.0, y), &CGSize::new(slider_w, 20.0)),
            200.0,
            1500.0,
            delay_ms,
        );
        button_set_action(delay_slider, action_handler, sel!(onDelayChanged:));
        add_subview(container, delay_slider);

        let delay_value = create_label(LabelConfig {
            frame: CGRect::new(
                &CGPoint::new(pad + 124.0 + slider_w + value_gap, y - 1.0),
                &CGSize::new(value_w, 20.0),
            ),
            text: format!("{} ms", delay_ms.round() as u64),
            font_size: ui_tokens::SMALL_FONT_SIZE,
            text_color: secondary,
            ..Default::default()
        });
        add_subview(container, delay_value);
        state.hold_delay_value_label = Some(delay_value as usize);
        y -= 20.0 + gap;

        let double_tap_label = create_label(LabelConfig {
            frame: CGRect::new(&CGPoint::new(pad, y), &CGSize::new(160.0, 20.0)),
            text: "Double-tap interval (ms):".to_string(),
            font_size: ui_tokens::SMALL_FONT_SIZE,
            text_color: secondary,
            ..Default::default()
        });
        add_subview(container, double_tap_label);

        let double_tap_ms = config.double_tap_interval_ms as f64;
        let double_tap_slider_w = (content_w - 164.0 - value_gap - value_w).max(120.0);
        let double_tap_slider = create_slider(
            CGRect::new(
                &CGPoint::new(pad + 164.0, y),
                &CGSize::new(double_tap_slider_w, 20.0),
            ),
            100.0,
            450.0,
            double_tap_ms,
        );
        button_set_action(
            double_tap_slider,
            action_handler,
            sel!(onDoubleTapIntervalChanged:),
        );
        add_subview(container, double_tap_slider);

        let double_tap_value = create_label(LabelConfig {
            frame: CGRect::new(
                &CGPoint::new(pad + 164.0 + double_tap_slider_w + value_gap, y - 1.0),
                &CGSize::new(value_w, 20.0),
            ),
            text: format!("{} ms", double_tap_ms.round() as u64),
            font_size: ui_tokens::SMALL_FONT_SIZE,
            text_color: secondary,
            ..Default::default()
        });
        add_subview(container, double_tap_value);
        state.double_tap_value_label = Some(double_tap_value as usize);

        state.keys_mode_binding_labels = mode_binding_labels;
        state.keys_recorder_hint_label = Some(recorder_hint as usize);
        state.keys_conflict_label = Some(conflict_label as usize);
        state.keys_conflict_details_button = Some(conflict_details_button as usize);

        container
    } // unsafe
}

// ============================================================================
// AI & Prompts tab
// ============================================================================
