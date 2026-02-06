use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::sync::Mutex;
use std::thread;
use std::time::Duration;

use core_graphics::geometry::{CGPoint, CGRect, CGSize};
use dispatch::Queue;
use lazy_static::lazy_static;
use objc::runtime::{Class, Object};
use objc::{msg_send, sel, sel_impl};
use objc2_app_kit::{
    NSVisualEffectBlendingMode, NSVisualEffectMaterial, NSVisualEffectState,
    NSWindowCollectionBehavior,
};
use tracing::{info, warn};

use crate::config::{Config, HoldMods, ToggleTrigger};
use crate::ipc::{IpcCommand, IpcResponse};
use crate::os::hotkeys;
use crate::tray::{TrayMenuEvent, send_menu_event};
use crate::ui::bootstrap::handlers::{action_handler_class, window_delegate_class};
use crate::ui_helpers::{
    LabelConfig, add_subview, button, button_set_action, create_checkbox, create_floating_window,
    create_label, create_secure_text_input, create_slider, create_text_input, ns_string,
    set_text_field_string, set_visual_effect_blending, set_visual_effect_material,
    set_visual_effect_state, ui_colors, ui_tokens, window_close, window_content_view, window_show,
};

mod handlers;

// Type alias for Objective-C object pointers
type Id = *mut Object;

const SIDEBAR_WIDTH: f64 = 120.0;
const TAB_SETUP: usize = 0;
const _TAB_KEYS: usize = 1;
const _TAB_AUDIO: usize = 2;

const STEP_TEST_MIC: usize = 0;
const STEP_SHOW_OVERLAY: usize = 1;
const STEP_PRESS_HOTKEY: usize = 2;

#[derive(Default)]
struct BootstrapState {
    window: Option<usize>,
    window_delegate: Option<usize>,
    root_view: Option<usize>,
    step_labels: [Option<usize>; 3],
    tab_buttons: [Option<usize>; 3],
    content_views: [Option<usize>; 3],
    active_tab: usize,
    keys_hold_popup: Option<usize>,
    keys_toggle_popup: Option<usize>,
    keys_preset_popup: Option<usize>,
    keys_exclusive_checkbox: Option<usize>,
    hold_delay_value_label: Option<usize>,
    double_tap_value_label: Option<usize>,
    config_cache: Option<Config>,
    // Onboarding additions
    permission_labels: [Option<usize>; 3], // Mic, Accessibility, Input Monitoring
    quality_daemon_checkbox: Option<usize>,
    completion_view: Option<usize>,
    llm_endpoint_field: Option<usize>,
    llm_model_field: Option<usize>,
    llm_key_field: Option<usize>,
    assistive_endpoint_field: Option<usize>,
    assistive_model_field: Option<usize>,
    assistive_key_field: Option<usize>,
}

lazy_static! {
    static ref BOOTSTRAP_STATE: Mutex<BootstrapState> = Mutex::new(BootstrapState::default());
}

fn bootstrap_done_path() -> PathBuf {
    Config::config_dir().join("bootstrap_done")
}

pub fn should_show_bootstrap() -> bool {
    !bootstrap_done_path().exists()
}

fn mark_bootstrap_done() {
    let path = bootstrap_done_path();
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    let _ = fs::write(path, "done");
}

pub fn schedule_bootstrap() {
    if !should_show_bootstrap() {
        return;
    }

    thread::spawn(|| {
        thread::sleep(Duration::from_millis(800));
        show_bootstrap_overlay();
    });
}

pub fn show_bootstrap_overlay() {
    std::thread::spawn(|| {
        let config = Config::load();
        Queue::main().exec_async(move || {
            let mut state = BOOTSTRAP_STATE.lock().unwrap_or_else(|e| e.into_inner());
            state.config_cache = Some(config);
            drop(state);
            show_bootstrap_overlay_impl();
        });
    });
}

fn show_bootstrap_overlay_impl() {
    // Keep bootstrap as a standalone onboarding window.
    // It should not depend on the voice chat overlay being available.
    // (This also avoids deadlocks when the overlay is mid-layout.)
    unsafe {
        let reuse_window = {
            let mut state = BOOTSTRAP_STATE.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(window_ptr) = state.window {
                let ns_window = Class::get("NSWindow").unwrap();
                let window = window_ptr as Id;
                let is_window: bool = msg_send![window, isKindOfClass: ns_window];
                if is_window {
                    Some(window)
                } else {
                    state.window = None;
                    None
                }
            } else {
                None
            }
        }; // Release lock before AppKit call.
        if let Some(window) = reuse_window {
            window_show(window);
            return;
        }

        let ns_screen = Class::get("NSScreen").unwrap();
        let screen: Id = msg_send![ns_screen, mainScreen];
        if screen.is_null() {
            warn!("No NSScreen available for bootstrap window");
            return;
        }
        let visible: CGRect = msg_send![screen, visibleFrame];
        let window_width = 720.0;
        let window_height = 640.0;
        let x = visible.origin.x + (visible.size.width - window_width) * 0.5;
        let y = visible.origin.y + (visible.size.height - window_height) * 0.5;
        let frame = CGRect::new(
            &CGPoint::new(x, y),
            &CGSize::new(window_width, window_height),
        );

        let window = create_floating_window(frame, "Settings", false);
        let _: () = msg_send![window, setOpaque: false];
        let _: () = msg_send![window, setLevel: crate::ui_helpers::NS_NORMAL_WINDOW_LEVEL];
        let _: () = msg_send![window, setCollectionBehavior: NSWindowCollectionBehavior::empty()];
        let delegate_class = window_delegate_class();
        let window_delegate: Id = msg_send![delegate_class, new];
        let _: () = msg_send![window, setDelegate: window_delegate];
        let content_view = window_content_view(window);
        let bounds: CGRect = msg_send![content_view, bounds];
        let _ = attach_settings_view(content_view, bounds);

        {
            let mut state = BOOTSTRAP_STATE.lock().unwrap_or_else(|e| e.into_inner());
            state.window = Some(window as usize);
            state.window_delegate = Some(window_delegate as usize);
        } // Release lock before AppKit call to avoid nested-runloop deadlock.

        window_show(window);
    }
}

/// Attach the Settings view inside an existing parent view.
///
/// # Safety
/// `parent` must be a valid `NSView` instance owned by AppKit.
unsafe fn attach_settings_view(parent: Id, frame: core_graphics::geometry::CGRect) -> Option<Id> {
    unsafe {
        let (config, existing_root) = {
            let state = BOOTSTRAP_STATE.lock().unwrap_or_else(|e| e.into_inner());
            (
                state.config_cache.clone().unwrap_or_else(Config::load),
                state.root_view,
            )
        };
        if let Some(root_ptr) = existing_root {
            let root = root_ptr as Id;
            let _: () = msg_send![root, setFrame: frame];
            let _: () = msg_send![
                root,
                setAutoresizingMask: 2_isize | 16_isize // NSViewWidthSizable | NSViewHeightSizable
            ];
            let superview: Id = msg_send![root, superview];
            if superview.is_null() {
                add_subview(parent, root);
            }
            return Some(root);
        }

        let ns_visual = Class::get("NSVisualEffectView").unwrap();
        let root: Id = msg_send![ns_visual, alloc];
        let root: Id = msg_send![root, initWithFrame: frame];
        set_visual_effect_material(root, NSVisualEffectMaterial::WindowBackground);
        set_visual_effect_blending(root, NSVisualEffectBlendingMode::BehindWindow);
        set_visual_effect_state(root, NSVisualEffectState::Active);
        let _: () = msg_send![root, setWantsLayer: true];
        let _: () = msg_send![
            root,
            setAutoresizingMask: 2_isize | 16_isize // NSViewWidthSizable | NSViewHeightSizable
        ];
        let layer: Id = msg_send![root, layer];
        if !layer.is_null() {
            let _: () = msg_send![layer, setCornerRadius: 14.0f64];
            let _: () = msg_send![layer, setMasksToBounds: true];
        }
        add_subview(parent, root);

        let action_handler_class = action_handler_class();
        let action_handler: Id = msg_send![action_handler_class, new];
        let built_state = build_settings_ui(
            root,
            frame.size.width,
            frame.size.height,
            action_handler,
            &config,
        );

        let mut state = BOOTSTRAP_STATE.lock().unwrap_or_else(|e| e.into_inner());
        state.root_view = Some(root as usize);
        state.window = None;
        state.step_labels = built_state.step_labels;
        state.tab_buttons = built_state.tab_buttons;
        state.content_views = built_state.content_views;
        state.active_tab = built_state.active_tab;
        state.keys_hold_popup = built_state.keys_hold_popup;
        state.keys_toggle_popup = built_state.keys_toggle_popup;
        state.keys_preset_popup = built_state.keys_preset_popup;
        state.keys_exclusive_checkbox = built_state.keys_exclusive_checkbox;
        state.config_cache = built_state.config_cache;
        state.permission_labels = built_state.permission_labels;
        state.quality_daemon_checkbox = built_state.quality_daemon_checkbox;
        state.completion_view = built_state.completion_view;
        state.llm_endpoint_field = built_state.llm_endpoint_field;
        state.llm_model_field = built_state.llm_model_field;
        state.llm_key_field = built_state.llm_key_field;
        state.assistive_endpoint_field = built_state.assistive_endpoint_field;
        state.assistive_model_field = built_state.assistive_model_field;
        state.assistive_key_field = built_state.assistive_key_field;

        Some(root)
    }
}

// ============================================================================
// Permission checks (macOS system APIs)
// ============================================================================

fn check_permissions() -> [bool; 3] {
    unsafe {
        // Mic: AVCaptureDevice authorizationStatusForMediaType:
        let mic_ok = if let Some(av_class) = Class::get("AVCaptureDevice") {
            let audio_type = ns_string("soun"); // AVMediaTypeAudio fourcc
            let status: isize = msg_send![av_class, authorizationStatusForMediaType: audio_type];
            status == 3 // AVAuthorizationStatusAuthorized
        } else {
            false
        };

        // Accessibility: AXIsProcessTrusted()
        unsafe extern "C" {
            fn AXIsProcessTrusted() -> bool;
        }
        let ax_ok = AXIsProcessTrusted();

        // Input Monitoring: CGPreflightListenEventAccess() (macOS 10.15+)
        unsafe extern "C" {
            fn CGPreflightListenEventAccess() -> bool;
        }
        let input_ok = CGPreflightListenEventAccess();

        [mic_ok, ax_ok, input_ok]
    }
}

fn permission_color(granted: bool) -> Id {
    if granted {
        // System green
        unsafe {
            let ns_color = Class::get("NSColor").unwrap();
            msg_send![ns_color, systemGreenColor]
        }
    } else {
        // System red
        unsafe {
            let ns_color = Class::get("NSColor").unwrap();
            msg_send![ns_color, systemRedColor]
        }
    }
}

pub(super) fn refresh_permission_indicators() {
    let perms = check_permissions();
    let names = ["Mic", "Accessibility", "Input"];

    Queue::main().exec_async(move || unsafe {
        let labels = {
            let state = BOOTSTRAP_STATE.lock().unwrap_or_else(|e| e.into_inner());
            state.permission_labels
        };
        for (i, granted) in perms.iter().enumerate() {
            if let Some(label_ptr) = labels[i] {
                let label = label_ptr as Id;
                let dot = if *granted { "\u{25CF}" } else { "\u{25CB}" }; // ● vs ○
                let text = format!("{} {}", dot, names[i]);
                set_text_field_string(label, &text);
                let color = permission_color(*granted);
                let _: () = msg_send![label, setTextColor: color];
            }
        }
    });
}

unsafe fn build_settings_ui(
    root_view: Id,
    settings_width: f64,
    settings_height: f64,
    action_handler: Id,
    config: &Config,
) -> BootstrapState {
    unsafe {
        use core_graphics::geometry::{CGPoint, CGRect, CGSize};
        let ns_view = Class::get("NSView").unwrap();
        let mut state = BootstrapState::default();

        let settings_width = settings_width.max(SIDEBAR_WIDTH + 240.0);
        let settings_height = settings_height.max(280.0);
        let header_h = 88.0;
        let content_x = SIDEBAR_WIDTH;
        let content_width = (settings_width - SIDEBAR_WIDTH).max(240.0);
        let content_h = (settings_height - header_h).max(240.0);

        // ====================================================================
        // Title: "Welcome to CodeScribe"
        // ====================================================================
        let title_label = create_label(LabelConfig {
            frame: CGRect::new(
                &CGPoint::new(20.0, settings_height - 34.0),
                &CGSize::new(settings_width - 40.0, 28.0),
            ),
            text: "Welcome to CodeScribe".to_string(),
            font_size: 18.0,
            bold: true,
            text_color: crate::ui_helpers::color_label(),
            background_color: None,
            selectable: false,
            editable: false,
        });
        add_subview(root_view, title_label);

        let subtitle_label = create_label(LabelConfig {
            frame: CGRect::new(
                &CGPoint::new(20.0, settings_height - 54.0),
                &CGSize::new(settings_width - 40.0, 16.0),
            ),
            text: "Native macOS speech-to-text with AI formatting".to_string(),
            font_size: ui_tokens::SMALL_FONT_SIZE,
            bold: false,
            text_color: crate::ui_helpers::color_secondary_label(),
            ..Default::default()
        });
        add_subview(root_view, subtitle_label);

        // ====================================================================
        // Sidebar (left, 120px wide, darker background)
        // ====================================================================
        let sidebar_frame = CGRect::new(
            &CGPoint::new(0.0, 0.0),
            &CGSize::new(SIDEBAR_WIDTH, content_h),
        );
        let sidebar: Id = msg_send![ns_view, alloc];
        let sidebar: Id = msg_send![sidebar, initWithFrame: sidebar_frame];
        let _: () = msg_send![sidebar, setWantsLayer: true];
        let sidebar_layer: Id = msg_send![sidebar, layer];
        if !sidebar_layer.is_null() {
            let sidebar_bg = ui_colors::sidebar_bg();
            let cg_color: Id = msg_send![sidebar_bg, CGColor];
            let _: () = msg_send![sidebar_layer, setBackgroundColor: cg_color];
        }
        add_subview(root_view, sidebar);

        // Sidebar tab buttons
        let tab_names = ["Setup", "Keys", "Audio"];
        let tab_sels = [sel!(onTabSetup:), sel!(onTabKeys:), sel!(onTabAudio:)];
        let mut tab_buttons: [Option<usize>; 3] = [None; 3];

        for (i, (name, sel)) in tab_names.iter().zip(tab_sels.iter()).enumerate() {
            let btn_y = content_h - 44.0 * (i as f64 + 1.0);
            let btn_frame =
                CGRect::new(&CGPoint::new(0.0, btn_y), &CGSize::new(SIDEBAR_WIDTH, 36.0));

            let tab_btn = create_sidebar_tab_button(btn_frame, name, i == TAB_SETUP);
            button_set_action(tab_btn, action_handler, *sel);
            add_subview(sidebar, tab_btn);
            tab_buttons[i] = Some(tab_btn as usize);
        }

        // ====================================================================
        // Content area views (one per tab)
        // ====================================================================
        let content_frame = CGRect::new(
            &CGPoint::new(content_x, 0.0),
            &CGSize::new(content_width, content_h),
        );

        // --- Setup tab (index 0) ---
        let setup_view: Id = msg_send![ns_view, alloc];
        let setup_view: Id = msg_send![setup_view, initWithFrame: content_frame];
        add_subview(root_view, setup_view);

        let pad = ui_tokens::EDGE_PADDING;
        let field_w = content_width - pad * 2.0;
        let primary = crate::ui_helpers::color_label();
        let secondary = crate::ui_helpers::color_secondary_label();
        let mut y = content_h - 20.0;
        let mono_font = crate::ui_helpers::monospace_font(ui_tokens::SMALL_FONT_SIZE);
        let mono_font_input = crate::ui_helpers::monospace_font(ui_tokens::BODY_FONT_SIZE);

        // ── Permission indicators ────────────────────────────────────
        let perms = check_permissions();
        let perm_names = ["Mic", "Accessibility", "Input"];
        let perm_w = 130.0;
        let mut perm_labels: [Option<usize>; 3] = [None; 3];

        for (i, (name, granted)) in perm_names.iter().zip(perms.iter()).enumerate() {
            let dot = if *granted { "\u{25CF}" } else { "\u{25CB}" };
            let text = format!("{} {}", dot, name);
            let lbl = create_label(LabelConfig {
                frame: CGRect::new(
                    &CGPoint::new(pad + perm_w * i as f64, y),
                    &CGSize::new(perm_w, 18.0),
                ),
                text,
                font_size: ui_tokens::SMALL_FONT_SIZE,
                bold: true,
                text_color: permission_color(*granted),
                ..Default::default()
            });
            let _: () = msg_send![lbl, setFont: mono_font];
            add_subview(setup_view, lbl);
            perm_labels[i] = Some(lbl as usize);
        }

        let refresh_btn = button(
            CGRect::new(
                &CGPoint::new(content_width - 100.0, y - 2.0),
                &CGSize::new(80.0, 22.0),
            ),
            "Refresh",
        );
        button_set_action(refresh_btn, action_handler, sel!(onRefreshPermissions:));
        add_subview(setup_view, refresh_btn);
        y -= 32.0;

        // ── Quick-start steps ────────────────────────────────────────
        let step_defs: [(&str, objc::runtime::Sel, &str); 3] = [
            ("1) Test mic", sel!(onTestMic:), "Test"),
            ("2) Show chat overlay", sel!(onShowOverlay:), "Show"),
            ("3) Press hotkey", sel!(onHotkeyDone:), "Done"),
        ];
        let mut step_status_labels: [Option<usize>; 3] = [None; 3];

        for (i, (label_text, sel, btn_text)) in step_defs.iter().enumerate() {
            let step_label = create_label(LabelConfig {
                frame: CGRect::new(&CGPoint::new(pad, y), &CGSize::new(200.0, 20.0)),
                text: label_text.to_string(),
                font_size: ui_tokens::BODY_FONT_SIZE,
                bold: true,
                text_color: primary,
                ..Default::default()
            });
            add_subview(setup_view, step_label);

            let status_lbl = create_label(LabelConfig {
                frame: CGRect::new(&CGPoint::new(pad + 210.0, y), &CGSize::new(80.0, 20.0)),
                text: "pending".to_string(),
                font_size: ui_tokens::SMALL_FONT_SIZE,
                text_color: secondary,
                ..Default::default()
            });
            let _: () = msg_send![status_lbl, setFont: mono_font];
            add_subview(setup_view, status_lbl);
            step_status_labels[i] = Some(status_lbl as usize);

            let step_btn = button(
                CGRect::new(
                    &CGPoint::new(content_width - 100.0, y - 2.0),
                    &CGSize::new(80.0, 24.0),
                ),
                btn_text,
            );
            button_set_action(step_btn, action_handler, *sel);
            add_subview(setup_view, step_btn);
            y -= 34.0;
        }
        y -= 6.0;

        // ── Formatting AI (optional) ─────────────────────────────────
        let _fmt_header = create_label(LabelConfig {
            frame: CGRect::new(&CGPoint::new(pad, y), &CGSize::new(field_w, 18.0)),
            text: "Formatting AI (optional)".to_string(),
            font_size: ui_tokens::SMALL_FONT_SIZE,
            bold: true,
            text_color: secondary,
            ..Default::default()
        });
        add_subview(setup_view, _fmt_header);
        y -= 26.0;

        let llm_endpoint_val = config.llm_endpoint.as_deref().unwrap_or("");
        let llm_endpoint_field = create_text_input(
            CGRect::new(&CGPoint::new(pad, y), &CGSize::new(field_w, 22.0)),
            "Endpoint (e.g. https://api.openai.com/v1/responses)",
            llm_endpoint_val,
        );
        let _: () = msg_send![llm_endpoint_field, setFont: mono_font_input];
        button_set_action(
            llm_endpoint_field,
            action_handler,
            sel!(onLlmEndpointChanged:),
        );
        add_subview(setup_view, llm_endpoint_field);
        state.llm_endpoint_field = Some(llm_endpoint_field as usize);
        y -= 28.0;

        let llm_model_val = std::env::var("LLM_MODEL").unwrap_or_default();
        let llm_model_field = create_text_input(
            CGRect::new(&CGPoint::new(pad, y), &CGSize::new(field_w, 22.0)),
            "Model (e.g. gpt-4.1-mini)",
            &llm_model_val,
        );
        let _: () = msg_send![llm_model_field, setFont: mono_font_input];
        button_set_action(llm_model_field, action_handler, sel!(onLlmModelChanged:));
        add_subview(setup_view, llm_model_field);
        state.llm_model_field = Some(llm_model_field as usize);
        y -= 28.0;

        let llm_key_field = create_secure_text_input(
            CGRect::new(&CGPoint::new(pad, y), &CGSize::new(field_w, 22.0)),
            "API Key (stored in Keychain)",
        );
        let _: () = msg_send![llm_key_field, setFont: mono_font_input];
        button_set_action(llm_key_field, action_handler, sel!(onLlmKeyChanged:));
        add_subview(setup_view, llm_key_field);
        state.llm_key_field = Some(llm_key_field as usize);
        y -= 34.0;

        // ── Assistive AI (optional) ──────────────────────────────────
        let _assist_header = create_label(LabelConfig {
            frame: CGRect::new(&CGPoint::new(pad, y), &CGSize::new(field_w, 18.0)),
            text: "Assistive AI (optional)".to_string(),
            font_size: ui_tokens::SMALL_FONT_SIZE,
            bold: true,
            text_color: secondary,
            ..Default::default()
        });
        add_subview(setup_view, _assist_header);
        y -= 26.0;

        let assist_endpoint_val = std::env::var("LLM_ASSISTIVE_ENDPOINT").unwrap_or_default();
        let assist_endpoint_field = create_text_input(
            CGRect::new(&CGPoint::new(pad, y), &CGSize::new(field_w, 22.0)),
            "Endpoint (e.g. https://api.openai.com/v1/responses)",
            &assist_endpoint_val,
        );
        let _: () = msg_send![assist_endpoint_field, setFont: mono_font_input];
        button_set_action(
            assist_endpoint_field,
            action_handler,
            sel!(onAssistiveEndpointChanged:),
        );
        add_subview(setup_view, assist_endpoint_field);
        state.assistive_endpoint_field = Some(assist_endpoint_field as usize);
        y -= 28.0;

        let assist_model_val = std::env::var("LLM_ASSISTIVE_MODEL").unwrap_or_default();
        let assist_model_field = create_text_input(
            CGRect::new(&CGPoint::new(pad, y), &CGSize::new(field_w, 22.0)),
            "Model (e.g. gpt-5.2)",
            &assist_model_val,
        );
        let _: () = msg_send![assist_model_field, setFont: mono_font_input];
        button_set_action(
            assist_model_field,
            action_handler,
            sel!(onAssistiveModelChanged:),
        );
        add_subview(setup_view, assist_model_field);
        state.assistive_model_field = Some(assist_model_field as usize);
        y -= 28.0;

        let assist_key_field = create_secure_text_input(
            CGRect::new(&CGPoint::new(pad, y), &CGSize::new(field_w, 22.0)),
            "API Key (stored in Keychain)",
        );
        let _: () = msg_send![assist_key_field, setFont: mono_font_input];
        button_set_action(
            assist_key_field,
            action_handler,
            sel!(onAssistiveKeyChanged:),
        );
        add_subview(setup_view, assist_key_field);
        state.assistive_key_field = Some(assist_key_field as usize);
        y -= 34.0;

        let save_btn = button(
            CGRect::new(
                &CGPoint::new(content_width - 110.0, y + 4.0),
                &CGSize::new(90.0, 24.0),
            ),
            "Save",
        );
        button_set_action(save_btn, action_handler, sel!(onSaveApiSettings:));
        add_subview(setup_view, save_btn);
        y -= 34.0;

        // ── Quality daemon toggle ────────────────────────────────────
        let quality_on = std::env::var("CODESCRIBE_AUTOSTART_QUALITY_DAEMON")
            .map(|v| matches!(v.trim(), "1" | "true" | "yes" | "on"))
            .unwrap_or(false);
        let quality_check = create_checkbox(
            CGRect::new(&CGPoint::new(pad, y), &CGSize::new(field_w, 20.0)),
            "Auto-tune transcription quality (recommended)",
            quality_on,
        );
        button_set_action(quality_check, action_handler, sel!(onQualityDaemonToggled:));
        add_subview(setup_view, quality_check);
        y -= 18.0;

        let _quality_desc = create_label(LabelConfig {
            frame: CGRect::new(
                &CGPoint::new(pad + 22.0, y),
                &CGSize::new(field_w - 22.0, 16.0),
            ),
            text: "Runs quality analysis every 30min in background".to_string(),
            font_size: ui_tokens::MICRO_FONT_SIZE,
            text_color: secondary,
            ..Default::default()
        });
        add_subview(setup_view, _quality_desc);

        // ── Footer buttons ───────────────────────────────────────────
        let finish_btn = button(
            CGRect::new(
                &CGPoint::new(content_width - 110.0, 16.0),
                &CGSize::new(90.0, 28.0),
            ),
            "Finish",
        );
        button_set_action(finish_btn, action_handler, sel!(onFinish:));
        add_subview(setup_view, finish_btn);

        let skip_btn = button(
            CGRect::new(&CGPoint::new(pad, 16.0), &CGSize::new(90.0, 28.0)),
            "Skip",
        );
        button_set_action(skip_btn, action_handler, sel!(onFinish:));
        add_subview(setup_view, skip_btn);

        // ── Completion view (hidden, shown on Finish) ────────────────
        let completion: Id = msg_send![ns_view, alloc];
        let completion: Id = msg_send![completion, initWithFrame: content_frame];
        let _: () = msg_send![completion, setHidden: true];
        let done_label = create_label(LabelConfig {
            frame: CGRect::new(
                &CGPoint::new(0.0, content_h * 0.5 - 20.0),
                &CGSize::new(content_width, 40.0),
            ),
            text: "All set!".to_string(),
            font_size: 24.0,
            bold: true,
            text_color: permission_color(true), // green
            ..Default::default()
        });
        let _: () = msg_send![done_label, setAlignment: 1_isize]; // NSTextAlignmentCenter
        add_subview(completion, done_label);
        add_subview(root_view, completion);

        // --- Keys tab (index 1) ---
        let keys_view = build_keys_tab(action_handler, content_frame, config, &mut state);
        let _: () = msg_send![keys_view, setHidden: true];
        add_subview(root_view, keys_view);

        // --- Audio tab (index 2) ---
        let audio_view = build_audio_tab(action_handler, content_frame, config);
        let _: () = msg_send![audio_view, setHidden: true];
        add_subview(root_view, audio_view);

        // ====================================================================
        // Store state
        // ====================================================================
        state.step_labels = step_status_labels;
        state.tab_buttons = tab_buttons;
        state.content_views = [
            Some(setup_view as usize),
            Some(keys_view as usize),
            Some(audio_view as usize),
        ];
        state.active_tab = TAB_SETUP;
        state.permission_labels = perm_labels;
        state.quality_daemon_checkbox = Some(quality_check as usize);
        state.completion_view = Some(completion as usize);
        state.config_cache = Some(config.clone());

        state
    }
}

/// Create a sidebar tab button (flat, full-width, with highlight for active state).
unsafe fn create_sidebar_tab_button(
    frame: core_graphics::geometry::CGRect,
    title: &str,
    active: bool,
) -> Id {
    unsafe {
        let ns_button = Class::get("NSButton").unwrap();
        let ns_font = Class::get("NSFont").unwrap();

        let btn: Id = msg_send![ns_button, alloc];
        let btn: Id = msg_send![btn, initWithFrame: frame];

        let title_str = crate::ui_helpers::ns_string(title);
        let _: () = msg_send![btn, setTitle: title_str];
        let _: () = msg_send![btn, setBordered: false];
        let _: () = msg_send![
            btn,
            setFocusRingType: crate::ui_helpers::NS_FOCUS_RING_TYPE_NONE
        ];

        let font: Id = msg_send![ns_font, systemFontOfSize: ui_tokens::SMALL_FONT_SIZE];
        let _: () = msg_send![btn, setFont: font];

        let _: () = msg_send![btn, setWantsLayer: true];
        let layer: Id = msg_send![btn, layer];
        if !layer.is_null() {
            let bg = if active {
                ui_colors::panel_bg()
            } else {
                crate::ui_helpers::color_clear()
            };
            let cg_color: Id = msg_send![bg, CGColor];
            let _: () = msg_send![layer, setBackgroundColor: cg_color];
            let _: () = msg_send![layer, setCornerRadius: ui_tokens::CORNER_RADIUS_SM];
        }

        let tint = if active {
            crate::ui_helpers::color_label()
        } else {
            crate::ui_helpers::color_secondary_label()
        };
        let _: () = msg_send![btn, setContentTintColor: tint];

        btn
    }
}

/// Switch to a given tab index. Hides all content views, shows the selected one,
/// and updates sidebar button highlights.
pub(super) fn switch_tab(index: usize) {
    Queue::main().exec_async(move || unsafe {
        let (content_views, tab_buttons) = {
            let mut state = BOOTSTRAP_STATE.lock().unwrap_or_else(|e| e.into_inner());
            if index >= 3 || state.active_tab == index {
                return;
            }
            state.active_tab = index;
            (state.content_views, state.tab_buttons)
        };

        // Hide all content views, show selected
        for (i, cv) in content_views.iter().enumerate() {
            if let Some(ptr) = cv {
                let view = *ptr as Id;
                let _: () = msg_send![view, setHidden: (i != index)];
            }
        }

        // Update tab button highlights
        for (i, tb) in tab_buttons.iter().enumerate() {
            if let Some(ptr) = tb {
                let btn = *ptr as Id;
                let active = i == index;

                let _: () = msg_send![btn, setWantsLayer: true];
                let layer: Id = msg_send![btn, layer];
                if !layer.is_null() {
                    let bg = if active {
                        ui_colors::panel_bg()
                    } else {
                        crate::ui_helpers::color_clear()
                    };
                    let cg_color: Id = msg_send![bg, CGColor];
                    let _: () = msg_send![layer, setBackgroundColor: cg_color];
                    let _: () = msg_send![layer, setCornerRadius: ui_tokens::CORNER_RADIUS_SM];
                }

                let tint = if active {
                    crate::ui_helpers::color_label()
                } else {
                    crate::ui_helpers::color_secondary_label()
                };
                let _: () = msg_send![btn, setContentTintColor: tint];
            }
        }
    });
}

pub(super) fn handle_test_mic() {
    update_step_status(STEP_TEST_MIC, "recording\u{2026}");

    if let Err(e) = send_ipc(IpcCommand::StartRecording { assistive: false }) {
        warn!("Bootstrap test mic failed to start: {}", e);
        update_step_status(STEP_TEST_MIC, "failed");
        return;
    }

    thread::spawn(|| {
        thread::sleep(Duration::from_secs(3));
        let _ = send_ipc(IpcCommand::StopRecording);
        update_step_status(STEP_TEST_MIC, "done");
    });
}

pub(super) fn handle_show_overlay() {
    crate::show_voice_chat_overlay();
    crate::show_agent_tab();
    crate::voice_chat_ui::update_voice_chat_status("Listening...");
    update_step_status(STEP_SHOW_OVERLAY, "done");
}

pub(super) fn handle_hotkey_done() {
    update_step_status(STEP_PRESS_HOTKEY, "done");
}

pub(super) fn handle_finish() {
    // Show "All set!" completion view, then close after a brief delay.
    Queue::main().exec_async(|| unsafe {
        let (setup_ptr, tab_buttons, completion_ptr) = {
            let state = BOOTSTRAP_STATE.lock().unwrap_or_else(|e| e.into_inner());
            (
                state.content_views[0],
                state.tab_buttons,
                state.completion_view,
            )
        };
        if let Some(setup_ptr) = setup_ptr {
            let _: () = msg_send![setup_ptr as Id, setHidden: true];
        }
        // Hide sidebar tabs too
        for ptr in tab_buttons.iter().flatten() {
            let _: () = msg_send![*ptr as Id, setHidden: true];
        }
        if let Some(completion_ptr) = completion_ptr {
            let _: () = msg_send![completion_ptr as Id, setHidden: false];
        }
    });

    thread::spawn(|| {
        thread::sleep(Duration::from_millis(1200));
        mark_bootstrap_done();
        crate::voice_chat_ui::show_agent_tab();
        hide_bootstrap_overlay();
    });
}

pub(super) fn handle_bootstrap_window_closed() {
    let mut state = BOOTSTRAP_STATE.lock().unwrap_or_else(|e| e.into_inner());
    state.window = None;
    state.window_delegate = None;
    state.root_view = None;
    state.step_labels = [None, None, None];
    state.tab_buttons = [None, None, None];
    state.content_views = [None, None, None];
    state.keys_hold_popup = None;
    state.keys_toggle_popup = None;
    state.keys_preset_popup = None;
    state.keys_exclusive_checkbox = None;
    state.hold_delay_value_label = None;
    state.double_tap_value_label = None;
    state.permission_labels = [None, None, None];
    state.quality_daemon_checkbox = None;
    state.completion_view = None;
    state.llm_endpoint_field = None;
    state.llm_model_field = None;
    state.llm_key_field = None;
    state.assistive_endpoint_field = None;
    state.assistive_model_field = None;
    state.assistive_key_field = None;
    state.config_cache = None;
}

pub fn hide_bootstrap_overlay() {
    Queue::main().exec_async(|| unsafe {
        let (window_ptr, root_ptr) = {
            let mut state = BOOTSTRAP_STATE.lock().unwrap_or_else(|e| e.into_inner());
            let window_ptr = state.window.take();
            if window_ptr.is_some() {
                state.window_delegate = None;
                state.root_view = None;
                state.step_labels = [None, None, None];
                state.tab_buttons = [None, None, None];
                state.content_views = [None, None, None];
                state.keys_hold_popup = None;
                state.keys_toggle_popup = None;
                state.keys_preset_popup = None;
                state.keys_exclusive_checkbox = None;
                state.hold_delay_value_label = None;
                state.double_tap_value_label = None;
                state.permission_labels = [None, None, None];
                state.quality_daemon_checkbox = None;
                state.completion_view = None;
                state.llm_endpoint_field = None;
                state.llm_model_field = None;
                state.llm_key_field = None;
                state.assistive_endpoint_field = None;
                state.assistive_model_field = None;
                state.assistive_key_field = None;
                (window_ptr, None)
            } else {
                (None, state.root_view)
            }
        };

        if let Some(window_ptr) = window_ptr {
            window_close(window_ptr as Id);
            return;
        }

        if let Some(root_ptr) = root_ptr {
            let _: () = msg_send![root_ptr as Id, setHidden: true];
        }
    });
}

/// Reset embedded Settings view state when the overlay is destroyed.
pub fn reset_embedded_bootstrap_state() {
    let mut state = BOOTSTRAP_STATE.lock().unwrap_or_else(|e| e.into_inner());
    if state.window.is_some() {
        return;
    }
    state.root_view = None;
    state.window_delegate = None;
    state.config_cache = None;
    state.step_labels = [None, None, None];
    state.tab_buttons = [None, None, None];
    state.content_views = [None, None, None];
    state.keys_hold_popup = None;
    state.keys_toggle_popup = None;
    state.keys_preset_popup = None;
    state.keys_exclusive_checkbox = None;
    state.hold_delay_value_label = None;
    state.double_tap_value_label = None;
    state.permission_labels = [None, None, None];
    state.quality_daemon_checkbox = None;
    state.completion_view = None;
    state.llm_endpoint_field = None;
    state.llm_model_field = None;
    state.llm_key_field = None;
    state.assistive_endpoint_field = None;
    state.assistive_model_field = None;
    state.assistive_key_field = None;
}

fn update_step_status(index: usize, text: &str) {
    let text = text.to_string();
    Queue::main().exec_async(move || unsafe {
        let state = BOOTSTRAP_STATE.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(label) = state.step_labels.get(index).and_then(|v| *v) {
            set_text_field_string(label as Id, &text);
        }
    });
}

fn set_keys_popup_index(popup: Option<usize>, index: isize) {
    if let Some(popup) = popup {
        unsafe {
            let popup = popup as Id;
            let _: () = msg_send![popup, selectItemAtIndex: index];
        }
    }
}

fn set_keys_checkbox_state(checkbox: Option<usize>, enabled: bool) {
    if let Some(checkbox) = checkbox {
        unsafe {
            let checkbox = checkbox as Id;
            let state_value: isize = if enabled { 1 } else { 0 };
            let _: () = msg_send![checkbox, setState: state_value];
        }
    }
}

fn mark_keys_preset_custom() {
    let state = BOOTSTRAP_STATE.lock().unwrap_or_else(|e| e.into_inner());
    set_keys_popup_index(state.keys_preset_popup, 2);
}

// ============================================================================
// Keys tab
// ============================================================================

unsafe fn build_keys_tab(
    action_handler: Id,
    frame: core_graphics::geometry::CGRect,
    config: &Config,
    state: &mut BootstrapState,
) -> Id {
    use core_graphics::geometry::{CGPoint, CGRect, CGSize};
    unsafe {
        let ns_view = Class::get("NSView").unwrap();
        let ns_popup = Class::get("NSPopUpButton").unwrap();

        let container: Id = msg_send![ns_view, alloc];
        let container: Id = msg_send![container, initWithFrame: frame];

        let pad = ui_tokens::EDGE_PADDING;
        let content_w = frame.size.width - pad * 2.0;
        let mut y = frame.size.height - 40.0;
        let primary = crate::ui_helpers::color_label();
        let secondary = crate::ui_helpers::color_secondary_label();

        // Section title
        let title = create_label(LabelConfig {
            frame: CGRect::new(&CGPoint::new(pad, y), &CGSize::new(content_w, 22.0)),
            text: "Hotkey Configuration".to_string(),
            font_size: ui_tokens::BODY_FONT_SIZE,
            bold: true,
            text_color: primary,
            ..Default::default()
        });
        add_subview(container, title);
        y -= 36.0;

        // Preset dropdown
        let preset_label = create_label(LabelConfig {
            frame: CGRect::new(&CGPoint::new(pad, y), &CGSize::new(120.0, 20.0)),
            text: "Hotkey preset:".to_string(),
            font_size: ui_tokens::SMALL_FONT_SIZE,
            text_color: secondary,
            ..Default::default()
        });
        add_subview(container, preset_label);

        let preset_popup: Id = msg_send![ns_popup, alloc];
        let preset_popup: Id = msg_send![preset_popup, initWithFrame:
            CGRect::new(&CGPoint::new(pad + 124.0, y - 2.0), &CGSize::new(content_w - 124.0, 24.0))
            pullsDown: false
        ];
        let preset_titles = ["Fn (recommended)", "Safe (no toggles)", "Custom"];
        for title in &preset_titles {
            let ns_title = ns_string(title);
            let _: () = msg_send![preset_popup, addItemWithTitle: ns_title];
        }
        let preset_idx: isize = if config.hold_mods == crate::config::HoldMods::Fn
            && config.toggle_trigger == crate::config::ToggleTrigger::DoubleOption
            && !config.hold_exclusive
        {
            0
        } else if config.toggle_trigger == crate::config::ToggleTrigger::None
            && config.hold_mods == crate::config::HoldMods::Fn
            && config.hold_exclusive
        {
            1
        } else {
            2
        };
        let _: () = msg_send![preset_popup, selectItemAtIndex: preset_idx];
        button_set_action(preset_popup, action_handler, sel!(onPresetChanged:));
        add_subview(container, preset_popup);
        y -= 40.0;

        // Hold base dropdown
        let hold_label = create_label(LabelConfig {
            frame: CGRect::new(&CGPoint::new(pad, y), &CGSize::new(120.0, 20.0)),
            text: "Hold base:".to_string(),
            font_size: ui_tokens::SMALL_FONT_SIZE,
            text_color: secondary,
            ..Default::default()
        });
        add_subview(container, hold_label);

        let hold_popup: Id = msg_send![ns_popup, alloc];
        let hold_popup: Id = msg_send![hold_popup, initWithFrame:
            CGRect::new(&CGPoint::new(pad + 124.0, y - 2.0), &CGSize::new(content_w - 124.0, 24.0))
            pullsDown: false
        ];
        for title in &[
            "Fn (Globe)",
            "Ctrl",
            "Ctrl+Option",
            "Ctrl+Shift",
            "Ctrl+Command",
        ] {
            let ns_title = ns_string(title);
            let _: () = msg_send![hold_popup, addItemWithTitle: ns_title];
        }
        let hold_idx: isize = match config.hold_mods.as_str() {
            "fn" => 0,
            "ctrl" => 1,
            "ctrl_alt" => 2,
            "ctrl_shift" => 3,
            "ctrl_cmd" => 4,
            _ => 0,
        };
        let _: () = msg_send![hold_popup, selectItemAtIndex: hold_idx];
        button_set_action(hold_popup, action_handler, sel!(onHoldModChanged:));
        add_subview(container, hold_popup);
        y -= 36.0;

        // Shift/Cmd modes toggle
        let modes_check = create_checkbox(
            CGRect::new(&CGPoint::new(pad, y), &CGSize::new(content_w, 20.0)),
            "Enable Shift/Cmd modes (Chat/Selection)",
            !config.hold_exclusive,
        );
        button_set_action(modes_check, action_handler, sel!(onHoldExclusiveChanged:));
        add_subview(container, modes_check);
        y -= 32.0;

        // Hands-off toggle dropdown
        let toggle_label = create_label(LabelConfig {
            frame: CGRect::new(&CGPoint::new(pad, y), &CGSize::new(120.0, 20.0)),
            text: "Hands-off toggle:".to_string(),
            font_size: ui_tokens::SMALL_FONT_SIZE,
            text_color: secondary,
            ..Default::default()
        });
        add_subview(container, toggle_label);

        let toggle_popup: Id = msg_send![ns_popup, alloc];
        let toggle_popup: Id = msg_send![toggle_popup, initWithFrame:
            CGRect::new(&CGPoint::new(pad + 124.0, y - 2.0), &CGSize::new(content_w - 124.0, 24.0))
            pullsDown: false
        ];
        let toggle_titles = [
            "Off",
            "Double Ctrl (RAW)",
            "Left Option (normal)",
            "Right Option (assistive)",
            "Option keys (left=format, right=assistive)",
        ];
        for title in &toggle_titles {
            let ns_title = ns_string(title);
            let _: () = msg_send![toggle_popup, addItemWithTitle: ns_title];
        }
        let toggle_idx: isize = match config.toggle_trigger {
            crate::config::ToggleTrigger::None => 0,
            crate::config::ToggleTrigger::DoubleCtrl => 1,
            crate::config::ToggleTrigger::DoubleLeftOption => 2,
            crate::config::ToggleTrigger::DoubleRightOption => 3,
            crate::config::ToggleTrigger::DoubleOption => 4,
        };
        let _: () = msg_send![toggle_popup, selectItemAtIndex: toggle_idx];
        button_set_action(toggle_popup, action_handler, sel!(onToggleTriggerChanged:));
        add_subview(container, toggle_popup);
        y -= 44.0;

        // Hold start delay slider
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
        y -= 36.0;

        // Double-tap interval slider
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

        state.keys_hold_popup = Some(hold_popup as usize);
        state.keys_toggle_popup = Some(toggle_popup as usize);
        state.keys_preset_popup = Some(preset_popup as usize);
        state.keys_exclusive_checkbox = Some(modes_check as usize);
        state.hold_delay_value_label = Some(delay_value as usize);
        state.double_tap_value_label = Some(double_tap_value as usize);

        container
    } // unsafe
}

// ============================================================================
// Audio tab
// ============================================================================

unsafe fn build_audio_tab(
    action_handler: Id,
    frame: core_graphics::geometry::CGRect,
    config: &Config,
) -> Id {
    use core_graphics::geometry::{CGPoint, CGRect, CGSize};
    unsafe {
        let ns_view = Class::get("NSView").unwrap();
        let ns_popup = Class::get("NSPopUpButton").unwrap();

        let container: Id = msg_send![ns_view, alloc];
        let container: Id = msg_send![container, initWithFrame: frame];

        let pad = ui_tokens::EDGE_PADDING;
        let content_w = frame.size.width - pad * 2.0;
        let mut y = frame.size.height - 40.0;
        let primary = crate::ui_helpers::color_label();
        let secondary = crate::ui_helpers::color_secondary_label();

        // Section title
        let title = create_label(LabelConfig {
            frame: CGRect::new(&CGPoint::new(pad, y), &CGSize::new(content_w, 22.0)),
            text: "Audio & Transcription".to_string(),
            font_size: ui_tokens::BODY_FONT_SIZE,
            bold: true,
            text_color: primary,
            ..Default::default()
        });
        add_subview(container, title);
        y -= 36.0;

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
        y -= 38.0;

        // AI Formatting toggle
        let fmt_check = create_checkbox(
            CGRect::new(&CGPoint::new(pad, y), &CGSize::new(content_w, 20.0)),
            "AI Formatting",
            config.ai_formatting_enabled,
        );
        button_set_action(fmt_check, action_handler, sel!(onFormattingToggled:));
        add_subview(container, fmt_check);
        y -= 18.0;

        let fmt_desc = create_label(LabelConfig {
            frame: CGRect::new(
                &CGPoint::new(pad + 22.0, y),
                &CGSize::new(content_w - 22.0, 16.0),
            ),
            text: "Use LLM to clean up transcriptions".to_string(),
            font_size: ui_tokens::MICRO_FONT_SIZE,
            text_color: secondary,
            ..Default::default()
        });
        add_subview(container, fmt_desc);
        y -= 34.0;

        // VAD sensitivity dropdown
        let vad_label = create_label(LabelConfig {
            frame: CGRect::new(&CGPoint::new(pad, y), &CGSize::new(120.0, 18.0)),
            text: "VAD sensitivity:".to_string(),
            font_size: ui_tokens::SMALL_FONT_SIZE,
            text_color: secondary,
            ..Default::default()
        });
        add_subview(container, vad_label);

        let vad_popup: Id = msg_send![ns_popup, alloc];
        let vad_popup: Id = msg_send![vad_popup, initWithFrame:
            CGRect::new(&CGPoint::new(pad + 124.0, y - 2.0), &CGSize::new(240.0, 24.0))
            pullsDown: false
        ];
        let _: () = msg_send![vad_popup, addItemWithTitle: ns_string("Balanced")];
        let _: () = msg_send![vad_popup, addItemWithTitle: ns_string("Aggressive (less silence)")];
        let _: () =
            msg_send![vad_popup, addItemWithTitle: ns_string("Conservative (more context)")];
        let _: () = msg_send![vad_popup, selectItemAtIndex: 0_isize];
        button_set_action(vad_popup, action_handler, sel!(onVadPresetChanged:));
        add_subview(container, vad_popup);
        y -= 38.0;

        // Buffered streaming toggle
        let buffered_on = std::env::var("CODESCRIBE_BUFFERED_STREAM")
            .unwrap_or_default()
            .trim()
            == "1";
        let buf_check = create_checkbox(
            CGRect::new(&CGPoint::new(pad, y), &CGSize::new(content_w, 20.0)),
            "Backspace-magic streaming",
            buffered_on,
        );
        button_set_action(buf_check, action_handler, sel!(onBufferedToggled:));
        add_subview(container, buf_check);
        y -= 18.0;

        let buf_desc = create_label(LabelConfig {
            frame: CGRect::new(
                &CGPoint::new(pad + 22.0, y),
                &CGSize::new(content_w - 22.0, 16.0),
            ),
            text: "Progressive transcription with correction".to_string(),
            font_size: ui_tokens::MICRO_FONT_SIZE,
            text_color: secondary,
            ..Default::default()
        });
        add_subview(container, buf_desc);
        y -= 34.0;

        // Beep on start toggle
        let beep_check = create_checkbox(
            CGRect::new(&CGPoint::new(pad, y), &CGSize::new(content_w, 20.0)),
            "Beep on recording start",
            config.beep_on_start,
        );
        button_set_action(beep_check, action_handler, sel!(onBeepToggled:));
        add_subview(container, beep_check);
        y -= 34.0;

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
// Settings handler stubs (Keys + Audio tabs)
// ============================================================================

pub(super) extern "C" fn on_hold_mod_changed(_this: &Object, _cmd: objc::runtime::Sel, sender: Id) {
    unsafe {
        let idx: isize = msg_send![sender, indexOfSelectedItem];
        let (value, mods) = match idx {
            0 => ("fn", HoldMods::Fn),
            1 => ("ctrl", HoldMods::Ctrl),
            2 => ("ctrl_alt", HoldMods::CtrlAlt),
            3 => ("ctrl_shift", HoldMods::CtrlShift),
            4 => ("ctrl_cmd", HoldMods::CtrlCmd),
            _ => ("fn", HoldMods::Fn),
        };
        info!("Settings: hold modifier -> {}", value);
        let config = Config::load();
        let _ = config.save_to_env("HOLD_MODS", value);
        hotkeys::set_hold_mods(mods);
        send_menu_event(TrayMenuEvent::SetHoldMods(mods));

        // If DoubleCtrl toggle is enabled, Ctrl-only hold is unsafe → disable toggle.
        if mods == HoldMods::Ctrl && config.toggle_trigger == ToggleTrigger::DoubleCtrl {
            let _ = config.save_to_env("TOGGLE_TRIGGER", ToggleTrigger::None.as_str());
            hotkeys::set_toggle_trigger(ToggleTrigger::None);
            send_menu_event(TrayMenuEvent::SetToggleTrigger(ToggleTrigger::None));

            let state = BOOTSTRAP_STATE.lock().unwrap_or_else(|e| e.into_inner());
            set_keys_popup_index(state.keys_toggle_popup, 0);
        }

        mark_keys_preset_custom();
        crate::tray::refresh_hotkeys_menu_from_config();
    }
}

pub(super) extern "C" fn on_preset_changed(_this: &Object, _cmd: objc::runtime::Sel, sender: Id) {
    unsafe {
        let idx: isize = msg_send![sender, indexOfSelectedItem];
        match idx {
            // Fn (recommended)
            0 => {
                info!("Settings: hotkey preset -> fn_recommended");
                let config = Config::load();
                let _ = config.save_to_env("HOLD_MODS", HoldMods::Fn.as_str());
                let _ = config.save_to_env("TOGGLE_TRIGGER", ToggleTrigger::DoubleOption.as_str());
                let _ = config.save_to_env("HOLD_EXCLUSIVE", "0");
                hotkeys::set_hold_mods(HoldMods::Fn);
                hotkeys::set_toggle_trigger(ToggleTrigger::DoubleOption);
                hotkeys::set_exclusive_mode(false);
                send_menu_event(TrayMenuEvent::SetHoldMods(HoldMods::Fn));
                send_menu_event(TrayMenuEvent::SetToggleTrigger(ToggleTrigger::DoubleOption));
                send_menu_event(TrayMenuEvent::SetHoldExclusive(false));

                let state = BOOTSTRAP_STATE.lock().unwrap_or_else(|e| e.into_inner());
                set_keys_popup_index(state.keys_hold_popup, 0);
                set_keys_popup_index(state.keys_toggle_popup, 4);
                set_keys_checkbox_state(state.keys_exclusive_checkbox, true);
                crate::tray::refresh_hotkeys_menu_from_config();
            }
            // Safe (no toggles)
            1 => {
                info!("Settings: hotkey preset -> safe");
                let config = Config::load();
                let _ = config.save_to_env("HOLD_MODS", HoldMods::Fn.as_str());
                let _ = config.save_to_env("TOGGLE_TRIGGER", ToggleTrigger::None.as_str());
                let _ = config.save_to_env("HOLD_EXCLUSIVE", "1");
                hotkeys::set_hold_mods(HoldMods::Fn);
                hotkeys::set_toggle_trigger(ToggleTrigger::None);
                hotkeys::set_exclusive_mode(true);
                send_menu_event(TrayMenuEvent::SetHoldMods(HoldMods::Fn));
                send_menu_event(TrayMenuEvent::SetToggleTrigger(ToggleTrigger::None));
                send_menu_event(TrayMenuEvent::SetHoldExclusive(true));

                let state = BOOTSTRAP_STATE.lock().unwrap_or_else(|e| e.into_inner());
                set_keys_popup_index(state.keys_hold_popup, 0);
                set_keys_popup_index(state.keys_toggle_popup, 0);
                set_keys_checkbox_state(state.keys_exclusive_checkbox, false);
                crate::tray::refresh_hotkeys_menu_from_config();
            }
            _ => {
                info!("Settings: hotkey preset -> custom");
                crate::tray::refresh_hotkeys_menu_from_config();
            }
        }
    }
}

pub(super) extern "C" fn on_hold_exclusive_changed(
    _this: &Object,
    _cmd: objc::runtime::Sel,
    sender: Id,
) {
    unsafe {
        let state: isize = msg_send![sender, state];
        let enabled = state == 1;
        let hold_exclusive = !enabled;
        info!("Settings: hold exclusive -> {}", hold_exclusive);
        let config = Config::load();
        let _ = config.save_to_env("HOLD_EXCLUSIVE", if hold_exclusive { "1" } else { "0" });
        hotkeys::set_exclusive_mode(hold_exclusive);
        send_menu_event(TrayMenuEvent::SetHoldExclusive(hold_exclusive));
        mark_keys_preset_custom();
        crate::tray::refresh_hotkeys_menu_from_config();
    }
}

pub(super) extern "C" fn on_toggle_trigger_changed(
    _this: &Object,
    _cmd: objc::runtime::Sel,
    sender: Id,
) {
    unsafe {
        let idx: isize = msg_send![sender, indexOfSelectedItem];
        let (trigger, value) = match idx {
            0 => (ToggleTrigger::None, "none"),
            1 => (ToggleTrigger::DoubleCtrl, "double_ctrl"),
            2 => (ToggleTrigger::DoubleLeftOption, "double_lalt"),
            3 => (ToggleTrigger::DoubleRightOption, "double_ralt"),
            4 => (ToggleTrigger::DoubleOption, "double_option"),
            _ => (ToggleTrigger::None, "none"),
        };
        info!("Settings: toggle trigger -> {}", value);
        let config = Config::load();
        let _ = config.save_to_env("TOGGLE_TRIGGER", value);
        hotkeys::set_toggle_trigger(trigger);
        send_menu_event(TrayMenuEvent::SetToggleTrigger(trigger));

        // If enabling DoubleCtrl and hold is Ctrl-only, switch to Ctrl+Option and enable modes.
        if trigger == ToggleTrigger::DoubleCtrl && config.hold_mods == HoldMods::Ctrl {
            let _ = config.save_to_env("HOLD_MODS", HoldMods::CtrlAlt.as_str());
            let _ = config.save_to_env("HOLD_EXCLUSIVE", "0");
            hotkeys::set_hold_mods(HoldMods::CtrlAlt);
            hotkeys::set_exclusive_mode(false);
            send_menu_event(TrayMenuEvent::SetHoldMods(HoldMods::CtrlAlt));
            send_menu_event(TrayMenuEvent::SetHoldExclusive(false));

            let state = BOOTSTRAP_STATE.lock().unwrap_or_else(|e| e.into_inner());
            set_keys_popup_index(state.keys_hold_popup, 1);
            set_keys_checkbox_state(state.keys_exclusive_checkbox, true);
        }

        mark_keys_preset_custom();
        crate::tray::refresh_hotkeys_menu_from_config();
    }
}

pub(super) extern "C" fn on_language_changed(_this: &Object, _cmd: objc::runtime::Sel, sender: Id) {
    unsafe {
        let idx: isize = msg_send![sender, indexOfSelectedItem];
        let lang = match idx {
            0 => "pl",
            1 => "en",
            _ => "pl",
        };
        info!("Settings: language -> {}", lang);
        let config = Config::load();
        let _ = config.save_to_env("WHISPER_LANGUAGE", lang);
    }
}

pub(super) extern "C" fn on_formatting_toggled(
    _this: &Object,
    _cmd: objc::runtime::Sel,
    sender: Id,
) {
    unsafe {
        let state: isize = msg_send![sender, state];
        let enabled = state == 1;
        info!("Settings: AI formatting -> {}", enabled);
        let config = Config::load();
        let _ = config.save_to_env("AI_FORMATTING_ENABLED", if enabled { "1" } else { "0" });
    }
}

pub(super) extern "C" fn on_vad_preset_changed(
    _this: &Object,
    _cmd: objc::runtime::Sel,
    sender: Id,
) {
    unsafe {
        let idx: isize = msg_send![sender, indexOfSelectedItem];
        let preset = match idx {
            0 => "balanced",
            1 => "aggressive",
            2 => "conservative",
            _ => "balanced",
        };
        info!("Settings: VAD preset -> {}", preset);
        let config = Config::load();
        let _ = config.save_to_env("VAD_PRESET", preset);
    }
}

pub(super) extern "C" fn on_llm_endpoint_changed(
    _this: &Object,
    _cmd: objc::runtime::Sel,
    sender: Id,
) {
    unsafe {
        let ns_val: Id = msg_send![sender, stringValue];
        let cstr: *const std::ffi::c_char = msg_send![ns_val, UTF8String];
        let value = std::ffi::CStr::from_ptr(cstr).to_string_lossy().to_string();
        info!("Settings: LLM endpoint -> {}", value);
        let config = Config::load();
        let _ = config.save_to_env("LLM_ENDPOINT", &value);
    }
}

pub(super) extern "C" fn on_llm_model_changed(
    _this: &Object,
    _cmd: objc::runtime::Sel,
    sender: Id,
) {
    unsafe {
        let ns_val: Id = msg_send![sender, stringValue];
        let cstr: *const std::ffi::c_char = msg_send![ns_val, UTF8String];
        let value = std::ffi::CStr::from_ptr(cstr).to_string_lossy().to_string();
        info!("Settings: LLM model -> {}", value);
        let config = Config::load();
        let _ = config.save_to_env("LLM_MODEL", &value);
    }
}

pub(super) extern "C" fn on_llm_key_changed(_this: &Object, _cmd: objc::runtime::Sel, sender: Id) {
    unsafe {
        let ns_val: Id = msg_send![sender, stringValue];
        let cstr: *const std::ffi::c_char = msg_send![ns_val, UTF8String];
        let value = std::ffi::CStr::from_ptr(cstr).to_string_lossy().to_string();
        if !value.is_empty() {
            info!("Settings: LLM API key updated (stored in Keychain)");
            let config = Config::load();
            let _ = config.save_to_env("LLM_API_KEY", &value);
        }
    }
}

pub(super) extern "C" fn on_save_api_settings(
    _this: &Object,
    _cmd: objc::runtime::Sel,
    _sender: Id,
) {
    let (llm_endpoint, llm_model, llm_key, assist_endpoint, assist_model, assist_key) = {
        let state = BOOTSTRAP_STATE.lock().unwrap_or_else(|e| e.into_inner());
        (
            state.llm_endpoint_field,
            state.llm_model_field,
            state.llm_key_field,
            state.assistive_endpoint_field,
            state.assistive_model_field,
            state.assistive_key_field,
        )
    };

    let config = Config::load();
    unsafe {
        if let Some(ptr) = llm_endpoint {
            let value = crate::ui_helpers::get_text_field_string(ptr as Id);
            let _ = config.save_to_env("LLM_ENDPOINT", value.trim());
        }
        if let Some(ptr) = llm_model {
            let value = crate::ui_helpers::get_text_field_string(ptr as Id);
            let _ = config.save_to_env("LLM_MODEL", value.trim());
        }
        if let Some(ptr) = llm_key {
            let value = crate::ui_helpers::get_text_field_string(ptr as Id);
            if !value.trim().is_empty() {
                let _ = config.save_to_env("LLM_API_KEY", value.trim());
            }
        }
        if let Some(ptr) = assist_endpoint {
            let value = crate::ui_helpers::get_text_field_string(ptr as Id);
            let _ = config.save_to_env("LLM_ASSISTIVE_ENDPOINT", value.trim());
        }
        if let Some(ptr) = assist_model {
            let value = crate::ui_helpers::get_text_field_string(ptr as Id);
            let _ = config.save_to_env("LLM_ASSISTIVE_MODEL", value.trim());
        }
        if let Some(ptr) = assist_key {
            let value = crate::ui_helpers::get_text_field_string(ptr as Id);
            if !value.trim().is_empty() {
                let _ = config.save_to_env("LLM_ASSISTIVE_API_KEY", value.trim());
            }
        }
    }
    info!("Settings: API settings saved");
}

pub(super) extern "C" fn on_delay_changed(_this: &Object, _cmd: objc::runtime::Sel, sender: Id) {
    unsafe {
        let value: f64 = msg_send![sender, doubleValue];
        let ms = value.round() as u64;
        info!("Settings: hold delay -> {}ms", ms);
        let config = Config::load();
        let _ = config.save_to_env("HOLD_START_DELAY_MS", &ms.to_string());
        let label_ptr = {
            let state = BOOTSTRAP_STATE.lock().unwrap_or_else(|e| e.into_inner());
            state.hold_delay_value_label
        };
        if let Some(ptr) = label_ptr {
            set_text_field_string(ptr as Id, &format!("{ms} ms"));
        }
    }
}

pub(super) extern "C" fn on_double_tap_interval_changed(
    _this: &Object,
    _cmd: objc::runtime::Sel,
    sender: Id,
) {
    unsafe {
        let value: f64 = msg_send![sender, doubleValue];
        let ms = value.round() as u64;
        info!("Settings: double-tap interval -> {}ms", ms);
        let config = Config::load();
        let _ = config.save_to_env("DOUBLE_TAP_INTERVAL_MS", &ms.to_string());
        hotkeys::set_double_tap_interval_ms(ms);
        let label_ptr = {
            let state = BOOTSTRAP_STATE.lock().unwrap_or_else(|e| e.into_inner());
            state.double_tap_value_label
        };
        if let Some(ptr) = label_ptr {
            set_text_field_string(ptr as Id, &format!("{ms} ms"));
        }
    }
}

pub(super) extern "C" fn on_beep_toggled(_this: &Object, _cmd: objc::runtime::Sel, sender: Id) {
    unsafe {
        let state: isize = msg_send![sender, state];
        let enabled = state == 1;
        info!("Settings: beep on start -> {}", enabled);
        let config = Config::load();
        let _ = config.save_to_env("BEEP_ON_START", if enabled { "1" } else { "0" });
    }
}

pub(super) extern "C" fn on_volume_changed(_this: &Object, _cmd: objc::runtime::Sel, sender: Id) {
    unsafe {
        let value: f64 = msg_send![sender, doubleValue];
        info!("Settings: sound volume -> {:.2}", value);
        let config = Config::load();
        let _ = config.save_to_env("SOUND_VOLUME", &format!("{:.2}", value));
    }
}

pub(super) extern "C" fn on_buffered_toggled(_this: &Object, _cmd: objc::runtime::Sel, sender: Id) {
    unsafe {
        let state: isize = msg_send![sender, state];
        let enabled = state == 1;
        info!("Settings: buffered streaming -> {}", enabled);
        let config = Config::load();
        let _ = config.save_to_env(
            "CODESCRIBE_BUFFERED_STREAM",
            if enabled { "1" } else { "0" },
        );
    }
}

// ============================================================================
// Assistive AI + Quality daemon + Permissions handlers
// ============================================================================

pub(super) extern "C" fn on_assistive_endpoint_changed(
    _this: &Object,
    _cmd: objc::runtime::Sel,
    sender: Id,
) {
    unsafe {
        let ns_val: Id = msg_send![sender, stringValue];
        let cstr: *const std::ffi::c_char = msg_send![ns_val, UTF8String];
        let value = std::ffi::CStr::from_ptr(cstr).to_string_lossy().to_string();
        info!("Settings: assistive endpoint -> {}", value);
        let config = Config::load();
        let _ = config.save_to_env("LLM_ASSISTIVE_ENDPOINT", &value);
    }
}

pub(super) extern "C" fn on_assistive_model_changed(
    _this: &Object,
    _cmd: objc::runtime::Sel,
    sender: Id,
) {
    unsafe {
        let ns_val: Id = msg_send![sender, stringValue];
        let cstr: *const std::ffi::c_char = msg_send![ns_val, UTF8String];
        let value = std::ffi::CStr::from_ptr(cstr).to_string_lossy().to_string();
        info!("Settings: assistive model -> {}", value);
        let config = Config::load();
        let _ = config.save_to_env("LLM_ASSISTIVE_MODEL", &value);
    }
}

pub(super) extern "C" fn on_assistive_key_changed(
    _this: &Object,
    _cmd: objc::runtime::Sel,
    sender: Id,
) {
    unsafe {
        let ns_val: Id = msg_send![sender, stringValue];
        let cstr: *const std::ffi::c_char = msg_send![ns_val, UTF8String];
        let value = std::ffi::CStr::from_ptr(cstr).to_string_lossy().to_string();
        if !value.is_empty() {
            info!("Settings: assistive API key updated (stored in Keychain)");
            let config = Config::load();
            let _ = config.save_to_env("LLM_ASSISTIVE_API_KEY", &value);
        }
    }
}

pub(super) extern "C" fn on_quality_daemon_toggled(
    _this: &Object,
    _cmd: objc::runtime::Sel,
    sender: Id,
) {
    unsafe {
        let state: isize = msg_send![sender, state];
        let enabled = state == 1;
        info!("Settings: quality daemon autostart -> {}", enabled);
        let config = Config::load();
        let _ = config.save_to_env(
            "CODESCRIBE_AUTOSTART_QUALITY_DAEMON",
            if enabled { "1" } else { "0" },
        );
    }
}

pub(super) extern "C" fn on_refresh_permissions(
    _this: &Object,
    _cmd: objc::runtime::Sel,
    _sender: Id,
) {
    info!("Settings: refreshing permission indicators");
    refresh_permission_indicators();
}

fn send_ipc(cmd: IpcCommand) -> Result<IpcResponse, String> {
    let socket_path = crate::ipc::socket_path();
    let mut stream =
        UnixStream::connect(socket_path).map_err(|e| format!("IPC connect failed: {e}"))?;
    let payload = serde_json::to_string(&cmd).map_err(|e| e.to_string())?;
    stream
        .write_all(payload.as_bytes())
        .map_err(|e| e.to_string())?;
    stream.write_all(b"\n").map_err(|e| e.to_string())?;

    let mut reader = BufReader::new(stream);
    let mut line = String::new();
    reader.read_line(&mut line).map_err(|e| e.to_string())?;

    serde_json::from_str::<IpcResponse>(&line).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serial_test::serial;

    #[test]
    #[serial]
    #[cfg(target_os = "macos")]
    fn attach_settings_view_builds_root_view() {
        if std::env::var("CODESCRIBE_UI_TESTS").is_err() {
            return;
        }
        unsafe {
            let ns_view = Class::get("NSView").unwrap();
            let parent: Id = msg_send![ns_view, alloc];
            let parent: Id = msg_send![
                parent,
                initWithFrame: core_graphics::geometry::CGRect::new(
                    &core_graphics::geometry::CGPoint::new(0.0, 0.0),
                    &core_graphics::geometry::CGSize::new(480.0, 320.0),
                )
            ];

            let frame = core_graphics::geometry::CGRect::new(
                &core_graphics::geometry::CGPoint::new(0.0, 0.0),
                &core_graphics::geometry::CGSize::new(480.0, 320.0),
            );
            let view = attach_settings_view(parent, frame);
            assert!(view.is_some());

            reset_embedded_bootstrap_state();
            let state = BOOTSTRAP_STATE.lock().unwrap_or_else(|e| e.into_inner());
            assert!(state.root_view.is_none());
        }
    }
}
