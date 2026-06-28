//! AppKit builder for the AI & Prompts settings tab.

use super::*;

pub(super) unsafe fn build_ai_prompts_tab(
    action_handler: Id,
    frame: core_graphics::geometry::CGRect,
    _config: &Config,
    state: &mut SettingsWindowState,
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
            text: "User".to_string(),
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
            text: "Slow-moving user choices, AI formatting preference, and prompt editing."
                .to_string(),
            font_size: ui_tokens::MICRO_FONT_SIZE,
            text_color: secondary,
            ..Default::default()
        });
        add_subview(container, subtitle);
        y -= 16.0 + gap;

        add_settings_group_card(container, pad - 10.0, y + 28.0, content_w + 20.0, 198.0);
        let user_header = create_label(LabelConfig {
            frame: CGRect::new(&CGPoint::new(pad, y), &CGSize::new(content_w, 18.0)),
            text: "User Toggles".to_string(),
            font_size: ui_tokens::SMALL_FONT_SIZE,
            bold: true,
            text_color: primary,
            ..Default::default()
        });
        add_subview(container, user_header);
        y -= 18.0 + gap;

        let _dock_check = add_toggle_row(
            container,
            action_handler,
            pad,
            &mut y,
            content_w,
            secondary,
            ToggleRowSpec {
                title: "Show Dock icon",
                checked: _config.show_dock_icon,
                action: sel!(onShowDockIconToggled:),
                description: Some("Keep Codescribe in the Dock after windows close."),
                tag: None,
                gap,
            },
        );

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
            content_w,
            secondary,
            ToggleRowSpec {
                title: "Start quality daemon automatically",
                checked: quality_on,
                action: sel!(onQubeDaemonToggled:),
                description: Some(
                    "Starts bundled `qube-daemon --daemon` immediately and on next Codescribe launch when the binary is installed.",
                ),
                tag: None,
                gap,
            },
        );
        state.qube_daemon_checkbox = Some(quality_check as usize);

        let ultra_on = std::env::var("CODESCRIBE_LOCAL_STT_FINAL_PASS")
            .map(|v| parse_env_bool(&v))
            .unwrap_or(false);
        let ultra_check = add_toggle_row(
            container,
            action_handler,
            pad,
            &mut y,
            content_w,
            secondary,
            ToggleRowSpec {
                title: "Local file-based final pass",
                checked: ultra_on,
                action: sel!(onUltraQualityToggled:),
                description: Some(
                    "Re-runs local Whisper on saved audio after capture ends to strengthen or downgrade the committed verdict.",
                ),
                tag: None,
                gap,
            },
        );
        state.ultra_quality_checkbox = Some(ultra_check as usize);

        let _tagging_check = add_toggle_row(
            container,
            action_handler,
            pad,
            &mut y,
            content_w,
            secondary,
            ToggleRowSpec {
                title: "Tag transcripts for AI agents",
                checked: _config.transcript_tagging_enabled,
                action: sel!(onTranscriptTaggingToggled:),
                description: Some(
                    "Pastes speech as <codescribe mode=\"dictation\" lang=\"pl\">... for agent-aware dictated input.",
                ),
                tag: None,
                gap,
            },
        );

        y = add_tafla_header_separator(container, pad, y, content_w);
        y -= ui_tokens::SECTION_GAP;

        add_settings_group_card(container, pad - 10.0, y + 28.0, content_w + 20.0, 104.0);
        let fmt_header = create_label(LabelConfig {
            frame: CGRect::new(&CGPoint::new(pad, y), &CGSize::new(content_w, 18.0)),
            text: "Formatting AI (optional)".to_string(),
            font_size: ui_tokens::SMALL_FONT_SIZE,
            bold: true,
            text_color: secondary,
            ..Default::default()
        });
        add_subview(container, fmt_header);
        y -= 18.0 + gap;

        let _fmt_check = add_toggle_row(
            container,
            action_handler,
            pad,
            &mut y,
            content_w,
            secondary,
            ToggleRowSpec {
                title: "AI Formatting",
                checked: _config.ai_formatting_enabled,
                action: sel!(onFormattingToggled:),
                description: Some(
                    "Uses the formatting model to clean up the committed transcript; raw transcript is preserved on fallback.",
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

        let section_divider = create_label(LabelConfig {
            frame: CGRect::new(&CGPoint::new(pad, y), &CGSize::new(content_w, 1.0)),
            text: String::new(),
            background_color: Some(ui_colors::surface_border()),
            ..Default::default()
        });
        let _: () = msg_send![section_divider, setAlphaValue: 0.9f64];
        add_subview(container, section_divider);
        y -= ui_tokens::SECTION_GAP;

        add_settings_group_card(container, pad - 10.0, y + 28.0, content_w + 20.0, 312.0);
        let prompt_header = create_label(LabelConfig {
            frame: CGRect::new(&CGPoint::new(pad, y), &CGSize::new(content_w, 18.0)),
            text: "Prompt Editor".to_string(),
            font_size: ui_tokens::SMALL_FONT_SIZE,
            bold: true,
            text_color: primary,
            ..Default::default()
        });
        add_subview(container, prompt_header);
        y -= 18.0 + gap;

        let prompt_subtitle = create_label(LabelConfig {
            frame: CGRect::new(&CGPoint::new(pad, y), &CGSize::new(content_w, 16.0)),
            text:
            "Primary flow is fully in-app: switch prompt type, edit, save, or reset to defaults."
                .to_string(),
            font_size: ui_tokens::MICRO_FONT_SIZE,
            text_color: secondary,
            ..Default::default()
        });
        add_subview(container, prompt_subtitle);
        y -= 16.0 + gap;

        let prompt_type_label = create_label(LabelConfig {
            frame: CGRect::new(&CGPoint::new(pad, y), &CGSize::new(106.0, 20.0)),
            text: "Prompt type:".to_string(),
            font_size: ui_tokens::SMALL_FONT_SIZE,
            text_color: secondary,
            ..Default::default()
        });
        add_subview(container, prompt_type_label);

        let prompt_type_popup: Id = msg_send![ns_popup, alloc];
        let prompt_type_popup: Id = msg_send![prompt_type_popup, initWithFrame:
            CGRect::new(&CGPoint::new(pad + 108.0, y - 2.0), &CGSize::new(208.0, 24.0))
            pullsDown: false
        ];
        let _: () = msg_send![prompt_type_popup, addItemWithTitle: ns_string("Formatting prompt")];
        let _: () = msg_send![prompt_type_popup, addItemWithTitle: ns_string("Assistive prompt")];
        let _: () = msg_send![prompt_type_popup, selectItemAtIndex: 0_isize];
        button_set_action(
            prompt_type_popup,
            action_handler,
            sel!(onPromptTypeChanged:),
        );
        add_subview(container, prompt_type_popup);
        state.prompt_type_popup = Some(prompt_type_popup as usize);

        let reset_btn_w = 112.0;
        let save_prompt_btn_w = 98.0;
        let load_btn_w = 82.0;
        let reset_btn_x = pad + content_w - reset_btn_w;
        let save_prompt_btn_x = reset_btn_x - 8.0 - save_prompt_btn_w;
        let load_btn_x = save_prompt_btn_x - 8.0 - load_btn_w;

        let load_btn = create_button(
            CGRect::new(
                &CGPoint::new(load_btn_x, y - 2.0),
                &CGSize::new(load_btn_w, 24.0),
            ),
            "Load",
            button_style::GLASS,
        );
        button_set_action(load_btn, action_handler, sel!(onPromptLoad:));
        add_subview(container, load_btn);

        let save_prompt_btn = create_button(
            CGRect::new(
                &CGPoint::new(save_prompt_btn_x, y - 2.0),
                &CGSize::new(save_prompt_btn_w, 24.0),
            ),
            "Save Prompt",
            button_style::GLASS,
        );
        button_set_action(save_prompt_btn, action_handler, sel!(onPromptSave:));
        add_subview(container, save_prompt_btn);

        let reset_prompt_btn = create_button(
            CGRect::new(
                &CGPoint::new(reset_btn_x, y - 2.0),
                &CGSize::new(reset_btn_w, 24.0),
            ),
            "Reset Default",
            button_style::GLASS,
        );
        button_set_action(reset_prompt_btn, action_handler, sel!(onPromptReset:));
        add_subview(container, reset_prompt_btn);
        y -= 24.0 + gap;

        let initial_type = "formatting";
        let initial_path = prompt_path_text(initial_type);
        let path_label = create_label(LabelConfig {
            frame: CGRect::new(&CGPoint::new(pad, y), &CGSize::new(content_w, 16.0)),
            text: format!("Path: {initial_path}"),
            font_size: ui_tokens::MICRO_FONT_SIZE,
            text_color: secondary,
            ..Default::default()
        });
        add_subview(container, path_label);
        state.prompt_path_label = Some(path_label as usize);
        y -= 16.0 + gap;

        let prompt_editor_layout = compute_prompt_editor_layout(y, gap);
        let editor_height = prompt_editor_layout.editor_height;
        let editor_y = prompt_editor_layout.editor_y;
        let (editor_scroll, editor_text_view) = create_scrollable_text_view(
            CGRect::new(
                &CGPoint::new(pad, editor_y),
                &CGSize::new(content_w, editor_height),
            ),
            true,
        );
        let editor_font = crate::ui_helpers::monospace_font(ui_tokens::SMALL_FONT_SIZE);
        let _: () = msg_send![editor_text_view, setFont: editor_font];
        let _: () = msg_send![editor_text_view, setRichText: false];
        let _: () = msg_send![editor_scroll, setDrawsBackground: true];
        let editor_bg = settings_input_paper_bg();
        let _: () = msg_send![editor_scroll, setBackgroundColor: editor_bg];
        add_subview(container, editor_scroll);
        state.prompt_editor_text_view = Some(editor_text_view as usize);

        let initial_prompt = load_prompt_content(initial_type).unwrap_or_else(|err| {
            warn!("Settings: failed to load initial prompt: {err}");
            String::new()
        });
        set_text_view_string(editor_text_view, &initial_prompt);

        let prompt_status = create_label(LabelConfig {
            frame: CGRect::new(
                &CGPoint::new(pad, prompt_editor_layout.status_y),
                &CGSize::new(content_w, PROMPT_EDITOR_STATUS_HEIGHT),
            ),
            text: "Formatting prompt loaded.".to_string(),
            font_size: ui_tokens::MICRO_FONT_SIZE,
            text_color: secondary,
            ..Default::default()
        });
        add_subview(container, prompt_status);
        state.prompt_status_label = Some(prompt_status as usize);

        container
    }
}

// ============================================================================
// Audio & Input tab
// ============================================================================
