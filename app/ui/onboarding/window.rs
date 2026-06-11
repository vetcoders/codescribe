//! Wizard window construction: NSWindow setup, glass background, sidebar,
//! and all static UI elements built once per onboarding session.

use core_graphics::geometry::{CGPoint, CGRect, CGSize};
use objc::runtime::Class;
use objc::{msg_send, sel, sel_impl};
use objc2_app_kit::{
    NSBackingStoreType, NSVisualEffectBlendingMode, NSVisualEffectMaterial, NSVisualEffectState,
    NSWindowButton, NSWindowCollectionBehavior, NSWindowStyleMask,
};
use tracing::warn;

use crate::ui::shared::helpers::{
    LabelConfig, add_subview, button, button_set_action, color_clear, color_label,
    color_secondary_label, create_glass_effect_view_with, create_label, create_secure_text_input,
    ns_string, window_show,
};

use super::Id;
use super::handlers::{action_handler_class, window_delegate_class};
use super::permission_flow::{PERMISSION_ORDER, refresh_all_permission_states_locked};
use super::render::render_current_step;
use super::session::{load_onboarding_progress, release_onboarding_lock, save_onboarding_progress};
use super::state::{
    ONBOARDING_STATE, OnboardingState, UiRefs, initial_hotkey_choice, initial_language_choice,
    mode_api_key_configured,
};
use super::steps::TOTAL_STEPS;
use super::widgets::{configure_label, create_radio_button, system_secondary_color};

pub(super) const WINDOW_WIDTH: f64 = 720.0;
pub(super) const WINDOW_HEIGHT: f64 = 540.0;

pub(super) fn launch_onboarding_window() {
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(show_onboarding_wizard_impl)) {
        Ok(true) => {}
        Ok(false) => {
            warn!("Onboarding wizard did not open");
            release_onboarding_lock();
        }
        Err(_) => {
            warn!("Onboarding wizard terminated with panic");
            release_onboarding_lock();
        }
    }
}

pub(super) fn is_main_thread() -> bool {
    unsafe {
        let ns_thread = Class::get("NSThread").unwrap();
        msg_send![ns_thread, isMainThread]
    }
}

fn show_onboarding_wizard_impl() -> bool {
    unsafe {
        let existing = {
            let state = ONBOARDING_STATE.lock().unwrap_or_else(|e| e.into_inner());
            state.window
        };
        if let Some(window_ptr) = existing {
            let window = window_ptr as Id;
            let ns_window = Class::get("NSWindow").unwrap();
            let valid: bool = msg_send![window, isKindOfClass: ns_window];
            if valid {
                window_show(window);
                return true;
            }
        }

        let Some(screen_class) = Class::get("NSScreen") else {
            warn!("Onboarding: NSScreen class missing");
            return false;
        };
        let screen: Id = msg_send![screen_class, mainScreen];
        if screen.is_null() {
            warn!("Onboarding: No main screen");
            return false;
        }

        let visible: CGRect = msg_send![screen, visibleFrame];
        let origin_x = visible.origin.x + (visible.size.width - WINDOW_WIDTH) * 0.5;
        let origin_y = visible.origin.y + (visible.size.height - WINDOW_HEIGHT) * 0.5;
        let frame = CGRect::new(
            &CGPoint::new(origin_x, origin_y),
            &CGSize::new(WINDOW_WIDTH, WINDOW_HEIGHT),
        );

        let ns_window = Class::get("NSWindow").unwrap();
        let window: Id = msg_send![ns_window, alloc];
        let style = NSWindowStyleMask::Titled | NSWindowStyleMask::FullSizeContentView;
        let window: Id = msg_send![
            window,
            initWithContentRect: frame
            styleMask: style
            backing: NSBackingStoreType::Buffered
            defer: false
        ];

        let _: () = msg_send![window, setTitle: ns_string("Welcome to CodeScribe")];
        let _: () = msg_send![window, setTitleVisibility: 1_isize]; // NSWindowTitleHidden
        let _: () = msg_send![window, setTitlebarAppearsTransparent: true];
        let _: () = msg_send![window, setOpaque: false];
        let _: () = msg_send![window, setBackgroundColor: color_clear()];
        let _: () = msg_send![window, setReleasedWhenClosed: false];
        let _: () = msg_send![window, setMovableByWindowBackground: true];
        let _: () =
            msg_send![window, setCollectionBehavior: NSWindowCollectionBehavior::FullScreenNone];
        let size = CGSize::new(WINDOW_WIDTH, WINDOW_HEIGHT);
        let _: () = msg_send![window, setContentMinSize: size];
        let _: () = msg_send![window, setContentMaxSize: size];

        let close_btn: Id = msg_send![window, standardWindowButton: NSWindowButton::CloseButton];
        if !close_btn.is_null() {
            let _: () = msg_send![close_btn, setHidden: false];
            let _: () = msg_send![close_btn, setEnabled: true];
        }

        let mini_btn: Id =
            msg_send![window, standardWindowButton: NSWindowButton::MiniaturizeButton];
        if !mini_btn.is_null() {
            let _: () = msg_send![mini_btn, setHidden: false];
            let _: () = msg_send![mini_btn, setEnabled: true];
        }

        let zoom_btn: Id = msg_send![window, standardWindowButton: NSWindowButton::ZoomButton];
        if !zoom_btn.is_null() {
            let _: () = msg_send![zoom_btn, setHidden: true];
            let _: () = msg_send![zoom_btn, setEnabled: false];
        }

        let action_handler_class = action_handler_class();
        let action_handler: Id = msg_send![action_handler_class, new];

        let delegate_class = window_delegate_class();
        let window_delegate: Id = msg_send![delegate_class, new];
        let _: () = msg_send![window, setDelegate: window_delegate];

        let content_view: Id = msg_send![window, contentView];
        let content_bounds: CGRect = msg_send![content_view, bounds];
        let background = create_glass_effect_view_with(
            content_bounds,
            NSVisualEffectMaterial::HUDWindow,
            NSVisualEffectBlendingMode::BehindWindow,
            NSVisualEffectState::Active,
        );
        let _: () = msg_send![background, setAutoresizingMask: 2_isize | 16_isize];
        add_subview(content_view, background);

        let ui = build_onboarding_ui(background, action_handler);

        let resume_step = load_onboarding_progress();

        {
            let mut state = ONBOARDING_STATE.lock().unwrap_or_else(|e| e.into_inner());
            *state = OnboardingState {
                window: Some(window as usize),
                window_delegate: Some(window_delegate as usize),
                action_handler: Some(action_handler as usize),
                step_index: resume_step,
                language: initial_language_choice(),
                hotkey_mode: initial_hotkey_choice(),
                requested_permissions: [false; 5],
                permission_states: [super::permission_flow::PermissionUiStatus::NotDetermined; 5],
                scheduled_auto_advance_step: None,
                full_disk_polling: false,
                closing_via_finish: false,
                api_key_configured: mode_api_key_configured(),
                ui,
            };
            refresh_all_permission_states_locked(&mut state);
        }
        save_onboarding_progress(resume_step);

        render_current_step();
        window_show(window);
        true
    }
}

fn build_onboarding_ui(root: Id, action_handler: Id) -> UiRefs {
    unsafe {
        let ns_view = Class::get("NSView").unwrap();
        let mut ui = UiRefs::default();

        const SIDEBAR_WIDTH: f64 = 204.0;
        let content_left = SIDEBAR_WIDTH + 22.0;
        let content_width = WINDOW_WIDTH - content_left - 22.0;
        let content_center = content_left + (content_width * 0.5);

        let sidebar_bg = create_glass_effect_view_with(
            CGRect::new(
                &CGPoint::new(0.0, 0.0),
                &CGSize::new(SIDEBAR_WIDTH, WINDOW_HEIGHT),
            ),
            NSVisualEffectMaterial::Sidebar,
            NSVisualEffectBlendingMode::BehindWindow,
            NSVisualEffectState::Active,
        );
        let _: () = msg_send![
            sidebar_bg,
            setAutoresizingMask: 16_isize | 2_isize // NSViewHeightSizable | NSViewWidthSizable
        ];
        add_subview(root, sidebar_bg);

        let content_bg = create_glass_effect_view_with(
            CGRect::new(
                &CGPoint::new(SIDEBAR_WIDTH, 0.0),
                &CGSize::new(WINDOW_WIDTH - SIDEBAR_WIDTH, WINDOW_HEIGHT),
            ),
            NSVisualEffectMaterial::HUDWindow,
            NSVisualEffectBlendingMode::BehindWindow,
            NSVisualEffectState::Active,
        );
        let _: () = msg_send![
            content_bg,
            setAutoresizingMask: 16_isize | 2_isize // NSViewHeightSizable | NSViewWidthSizable
        ];
        add_subview(root, content_bg);

        let sidebar: Id = msg_send![ns_view, alloc];
        let sidebar: Id = msg_send![
            sidebar,
            initWithFrame: CGRect::new(&CGPoint::new(0.0, 0.0), &CGSize::new(SIDEBAR_WIDTH, WINDOW_HEIGHT))
        ];
        let _: () = msg_send![sidebar, setAutoresizingMask: 16_isize | 2_isize]; // NSViewHeightSizable | NSViewWidthSizable
        add_subview(root, sidebar);

        let sidebar_title = create_label(LabelConfig {
            frame: CGRect::new(
                &CGPoint::new(18.0, 494.0),
                &CGSize::new(SIDEBAR_WIDTH - 28.0, 22.0),
            ),
            text: "Onboarding".to_string(),
            font_size: 13.0,
            bold: true,
            text_color: color_label(),
            ..Default::default()
        });
        configure_label(sidebar_title, false, false);
        add_subview(sidebar, sidebar_title);

        let divider = create_label(LabelConfig {
            frame: CGRect::new(
                &CGPoint::new(SIDEBAR_WIDTH - 1.0, 0.0),
                &CGSize::new(1.0, WINDOW_HEIGHT),
            ),
            text: String::new(),
            background_color: Some(system_secondary_color()),
            ..Default::default()
        });
        add_subview(root, divider);

        let mut sidebar_step_labels: [Option<usize>; TOTAL_STEPS] = [None; TOTAL_STEPS];
        let mut y = 460.0;
        for slot in &mut sidebar_step_labels {
            let label = create_label(LabelConfig {
                frame: CGRect::new(
                    &CGPoint::new(18.0, y),
                    &CGSize::new(SIDEBAR_WIDTH - 28.0, 20.0),
                ),
                text: String::new(),
                font_size: 12.0,
                text_color: color_secondary_label(),
                ..Default::default()
            });
            configure_label(label, false, false);
            add_subview(sidebar, label);
            *slot = Some(label as usize);
            y -= 34.0;
        }
        ui.sidebar_step_labels = sidebar_step_labels;

        let icon_label = create_label(LabelConfig {
            frame: CGRect::new(
                &CGPoint::new(content_center - 72.0, 410.0),
                &CGSize::new(144.0, 34.0),
            ),
            text: String::new(),
            font_size: 24.0,
            bold: true,
            text_color: color_label(),
            ..Default::default()
        });
        configure_label(icon_label, true, false);
        add_subview(root, icon_label);
        ui.icon_label = Some(icon_label as usize);

        let title_label = create_label(LabelConfig {
            frame: CGRect::new(
                &CGPoint::new(content_left, 378.0),
                &CGSize::new(content_width, 34.0),
            ),
            text: String::new(),
            font_size: 24.0,
            bold: true,
            text_color: color_label(),
            ..Default::default()
        });
        configure_label(title_label, true, false);
        add_subview(root, title_label);
        ui.title_label = Some(title_label as usize);

        let description_label = create_label(LabelConfig {
            frame: CGRect::new(
                &CGPoint::new(content_left + 8.0, 268.0),
                &CGSize::new(content_width - 16.0, 104.0),
            ),
            text: String::new(),
            font_size: 14.0,
            text_color: color_secondary_label(),
            ..Default::default()
        });
        configure_label(description_label, true, true);
        add_subview(root, description_label);
        ui.description_label = Some(description_label as usize);

        let status_label = create_label(LabelConfig {
            frame: CGRect::new(
                &CGPoint::new(content_center - 132.0, 244.0),
                &CGSize::new(264.0, 22.0),
            ),
            text: String::new(),
            font_size: 13.0,
            bold: true,
            text_color: color_secondary_label(),
            ..Default::default()
        });
        configure_label(status_label, true, false);
        add_subview(root, status_label);
        ui.status_label = Some(status_label as usize);

        let instruction_label = create_label(LabelConfig {
            frame: CGRect::new(
                &CGPoint::new(content_left + 8.0, 196.0),
                &CGSize::new(content_width - 16.0, 54.0),
            ),
            text: String::new(),
            font_size: 12.0,
            text_color: color_secondary_label(),
            ..Default::default()
        });
        configure_label(instruction_label, true, true);
        add_subview(root, instruction_label);
        ui.instruction_label = Some(instruction_label as usize);

        let language_view: Id = msg_send![ns_view, alloc];
        let language_view: Id = msg_send![
            language_view,
            initWithFrame: CGRect::new(
                &CGPoint::new(content_center - 160.0, 186.0),
                &CGSize::new(320.0, 88.0)
            )
        ];
        add_subview(root, language_view);
        ui.language_view = Some(language_view as usize);

        let language_en = create_radio_button(
            CGRect::new(&CGPoint::new(10.0, 52.0), &CGSize::new(300.0, 24.0)),
            "English",
            true,
        );
        let _: () = msg_send![language_en, setTag: 0_isize];
        button_set_action(language_en, action_handler, sel!(onLanguageSelected:));
        add_subview(language_view, language_en);
        ui.language_en_radio = Some(language_en as usize);

        let language_pl = create_radio_button(
            CGRect::new(&CGPoint::new(10.0, 22.0), &CGSize::new(300.0, 24.0)),
            "Polish",
            false,
        );
        let _: () = msg_send![language_pl, setTag: 1_isize];
        button_set_action(language_pl, action_handler, sel!(onLanguageSelected:));
        add_subview(language_view, language_pl);
        ui.language_pl_radio = Some(language_pl as usize);

        let api_view: Id = msg_send![ns_view, alloc];
        let api_view: Id = msg_send![
            api_view,
            initWithFrame: CGRect::new(
                &CGPoint::new(content_center - 190.0, 180.0),
                &CGSize::new(380.0, 92.0)
            )
        ];
        add_subview(root, api_view);
        ui.api_view = Some(api_view as usize);

        let api_key_field = create_secure_text_input(
            CGRect::new(&CGPoint::new(0.0, 46.0), &CGSize::new(380.0, 28.0)),
            "Enter your LLM API key",
        );
        add_subview(api_view, api_key_field);
        ui.api_key_field = Some(api_key_field as usize);

        let api_hint = create_label(LabelConfig {
            frame: CGRect::new(&CGPoint::new(0.0, 8.0), &CGSize::new(380.0, 30.0)),
            text: "Stored securely in macOS Keychain.".to_string(),
            font_size: 11.0,
            text_color: color_secondary_label(),
            ..Default::default()
        });
        configure_label(api_hint, true, true);
        add_subview(api_view, api_hint);
        ui.api_hint_label = Some(api_hint as usize);

        let hotkey_view: Id = msg_send![ns_view, alloc];
        let hotkey_view: Id = msg_send![
            hotkey_view,
            initWithFrame: CGRect::new(
                &CGPoint::new(content_center - 200.0, 164.0),
                &CGSize::new(400.0, 132.0)
            )
        ];
        add_subview(root, hotkey_view);
        ui.hotkey_view = Some(hotkey_view as usize);

        let hotkey_hold = create_radio_button(
            CGRect::new(&CGPoint::new(0.0, 90.0), &CGSize::new(390.0, 24.0)),
            "Dictation mode: Hold (Fn/Globe)",
            false,
        );
        let _: () = msg_send![hotkey_hold, setTag: 0_isize];
        button_set_action(hotkey_hold, action_handler, sel!(onHotkeySelected:));
        add_subview(hotkey_view, hotkey_hold);
        ui.hotkey_hold_radio = Some(hotkey_hold as usize);

        let hotkey_toggle = create_radio_button(
            CGRect::new(&CGPoint::new(0.0, 58.0), &CGSize::new(390.0, 24.0)),
            "Hands-off mode: Toggle (Double-tap Option)",
            false,
        );
        let _: () = msg_send![hotkey_toggle, setTag: 1_isize];
        button_set_action(hotkey_toggle, action_handler, sel!(onHotkeySelected:));
        add_subview(hotkey_view, hotkey_toggle);
        ui.hotkey_toggle_radio = Some(hotkey_toggle as usize);

        let hotkey_both = create_radio_button(
            CGRect::new(&CGPoint::new(0.0, 26.0), &CGSize::new(390.0, 24.0)),
            "Hybrid mode: Hold + Toggle",
            true,
        );
        let _: () = msg_send![hotkey_both, setTag: 2_isize];
        button_set_action(hotkey_both, action_handler, sel!(onHotkeySelected:));
        add_subview(hotkey_view, hotkey_both);
        ui.hotkey_both_radio = Some(hotkey_both as usize);

        let summary_view: Id = msg_send![ns_view, alloc];
        const SUMMARY_WIDTH: f64 = 376.0;
        const SUMMARY_HEIGHT: f64 = 212.0;
        let summary_view: Id = msg_send![
            summary_view,
            initWithFrame: CGRect::new(
                &CGPoint::new(content_center - (SUMMARY_WIDTH * 0.5), 146.0),
                &CGSize::new(SUMMARY_WIDTH, SUMMARY_HEIGHT)
            )
        ];
        add_subview(root, summary_view);
        ui.summary_view = Some(summary_view as usize);

        let mut summary_labels: [Option<usize>; 5] = [None; 5];
        let summary_line_height = 28.0;
        let summary_top = SUMMARY_HEIGHT - 26.0;
        for (idx, permission) in PERMISSION_ORDER.iter().enumerate() {
            let y = summary_top - (idx as f64 * summary_line_height);
            let label = create_label(LabelConfig {
                frame: CGRect::new(&CGPoint::new(0.0, y), &CGSize::new(360.0, 22.0)),
                text: permission.title().to_string(),
                font_size: 12.0,
                text_color: color_secondary_label(),
                ..Default::default()
            });
            configure_label(label, false, false);
            add_subview(summary_view, label);
            summary_labels[idx] = Some(label as usize);
        }
        ui.summary_permission_labels = summary_labels;

        let summary_config = create_label(LabelConfig {
            frame: CGRect::new(&CGPoint::new(0.0, 0.0), &CGSize::new(360.0, 50.0)),
            text: String::new(),
            font_size: 12.0,
            text_color: color_secondary_label(),
            ..Default::default()
        });
        configure_label(summary_config, false, true);
        add_subview(summary_view, summary_config);
        ui.summary_config_label = Some(summary_config as usize);

        let primary_w = 132.0;
        let skip_w = 106.0;
        let back_w = 90.0;
        let button_y = 16.0;
        let primary_x = WINDOW_WIDTH - 18.0 - primary_w;
        let skip_x = primary_x - 8.0 - skip_w;
        let back_x = skip_x - 8.0 - back_w;

        let primary_button = button(
            CGRect::new(
                &CGPoint::new(primary_x, button_y),
                &CGSize::new(primary_w, 32.0),
            ),
            "Continue",
        );
        button_set_action(primary_button, action_handler, sel!(onPrimaryAction:));
        add_subview(root, primary_button);
        ui.primary_button = Some(primary_button as usize);

        let back_button = button(
            CGRect::new(&CGPoint::new(back_x, button_y), &CGSize::new(back_w, 32.0)),
            "Back",
        );
        button_set_action(back_button, action_handler, sel!(onBackAction:));
        add_subview(root, back_button);
        ui.back_button = Some(back_button as usize);

        let skip_button = button(
            CGRect::new(&CGPoint::new(skip_x, button_y), &CGSize::new(skip_w, 32.0)),
            "Skip",
        );
        button_set_action(skip_button, action_handler, sel!(onSkipAction:));
        add_subview(root, skip_button);
        ui.skip_button = Some(skip_button as usize);

        let step_counter = create_label(LabelConfig {
            frame: CGRect::new(
                &CGPoint::new(18.0, 20.0),
                &CGSize::new(SIDEBAR_WIDTH - 28.0, 20.0),
            ),
            text: String::new(),
            font_size: 12.0,
            bold: true,
            text_color: color_secondary_label(),
            ..Default::default()
        });
        configure_label(step_counter, false, false);
        add_subview(sidebar, step_counter);
        ui.step_counter_label = Some(step_counter as usize);

        ui
    }
}
