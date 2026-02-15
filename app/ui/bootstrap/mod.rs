use std::collections::HashMap;
use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::Duration;

use core_graphics::geometry::{CGPoint, CGRect, CGSize};
use dispatch::Queue;
use lazy_static::lazy_static;
use objc::runtime::{Class, Object};
use objc::{msg_send, sel, sel_impl};
use objc2_app_kit::{NSVisualEffectMaterial, NSWindowButton, NSWindowCollectionBehavior};
use tracing::{info, warn};

use crate::config::{Config, HoldMods, ToggleTrigger, keychain};
use crate::ipc::{IpcCommand, IpcResponse};
use crate::os::hotkeys;
use crate::ui::bootstrap::handlers::{action_handler_class, window_delegate_class};
use crate::ui_helpers::{
    LabelConfig, add_subview, button, button_set_action, button_style, create_button,
    create_checkbox, create_floating_window, create_glass_effect_view_with, create_label,
    create_secure_text_input, create_slider, create_text_input, ns_string, set_button_symbol,
    set_text_field_string, set_tooltip, style_toolbar_icon_button, ui_colors, ui_tokens,
    window_close, window_content_view, window_show,
};

mod handlers;

// Type alias for Objective-C object pointers
type Id = *mut Object;

const SIDEBAR_WIDTH: f64 = 204.0;
const SETTINGS_WINDOW_WIDTH: f64 = 760.0;
const SETTINGS_WINDOW_HEIGHT: f64 = 660.0;
const SETTINGS_TOPBAR_HEIGHT: f64 = 54.0;
const SETTINGS_CONTENT_INSET_X: f64 = 20.0;
const SETTINGS_CONTENT_INSET_Y: f64 = 12.0;
const TAB_SETUP: usize = 0;
const TAB_KEYS: usize = 1;
const TAB_AUDIO: usize = 2;
const TAB_VOICE_LAB: usize = 3;
const TAB_ENGINE: usize = 4;
const TAB_COUNT: usize = 5;

const STEP_TEST_MIC: usize = 0;
const STEP_SHOW_OVERLAY: usize = 1;
const STEP_PRESS_HOTKEY: usize = 2;

#[derive(Clone, Copy, PartialEq, Eq)]
enum VoiceLabFieldKind {
    Bool,
    Value,
}

#[derive(Clone, Copy)]
struct VoiceLabFieldSpec {
    key: &'static str,
    label: &'static str,
    default_value: &'static str,
    description: &'static str,
    kind: VoiceLabFieldKind,
}

// Voice Lab: only fields where user choice actually improves UX.
// Tuned pipeline internals (chunk_sec, similarity, correction thresholds etc.)
// stay as env-var escape hatches — their defaults are proven optimal.
const VOICE_LAB_FIELDS: [VoiceLabFieldSpec; 7] = [
    VoiceLabFieldSpec {
        key: "CODESCRIBE_BUFFERED_STREAM",
        label: "Buffered streaming",
        default_value: "1",
        description: "Smoother, correction-capable streaming mode.",
        kind: VoiceLabFieldKind::Bool,
    },
    VoiceLabFieldSpec {
        key: "CODESCRIBE_BUFFER_DELAY_MS",
        label: "Buffer delay (ms)",
        default_value: "1800",
        description: "Delay before buffered emission starts.",
        kind: VoiceLabFieldKind::Value,
    },
    VoiceLabFieldSpec {
        key: "CODESCRIBE_TYPING_CPS",
        label: "Typing speed (CPS)",
        default_value: "36",
        description: "Characters-per-second animation speed.",
        kind: VoiceLabFieldKind::Value,
    },
    VoiceLabFieldSpec {
        key: "CODESCRIBE_EMIT_WORDS_MAX",
        label: "Emit words max",
        default_value: "3",
        description: "Max words emitted per tick in buffered mode.",
        kind: VoiceLabFieldKind::Value,
    },
    VoiceLabFieldSpec {
        key: "CODESCRIBE_BUFFERED_INTERIM_SEC",
        label: "Interim cadence (sec)",
        default_value: "3.0",
        description: "How often partial results are shown.",
        kind: VoiceLabFieldKind::Value,
    },
    VoiceLabFieldSpec {
        key: "WHISPER_MODEL",
        label: "Whisper cloud model",
        default_value: "mlx-community/whisper-large-v3-mlx",
        description: "Cloud/multipart STT model id.",
        kind: VoiceLabFieldKind::Value,
    },
    VoiceLabFieldSpec {
        key: "BACKEND_MAX_UPLOAD_MB",
        label: "Cloud upload cap (MB)",
        default_value: "20",
        description: "Max upload size for cloud STT multipart.",
        kind: VoiceLabFieldKind::Value,
    },
];

fn parse_env_bool(v: &str) -> bool {
    matches!(
        v.trim().to_ascii_lowercase().as_str(),
        "1" | "true" | "yes" | "on"
    )
}

fn voice_lab_value(spec: &VoiceLabFieldSpec) -> String {
    std::env::var(spec.key).unwrap_or_else(|_| spec.default_value.to_string())
}

fn voice_lab_value_from_snapshot(
    spec: &VoiceLabFieldSpec,
    env_snapshot: &HashMap<String, String>,
) -> String {
    env_snapshot
        .get(spec.key)
        .cloned()
        .unwrap_or_else(|| spec.default_value.to_string())
}

fn parse_ranged_f32(raw: &str, min: f32, max: f32) -> Option<f32> {
    let parsed = raw.parse::<f32>().ok()?;
    if !parsed.is_finite() || parsed < min || parsed > max {
        return None;
    }
    Some(parsed)
}

fn parse_ranged_u64(raw: &str, min: u64, max: u64) -> Option<u64> {
    let parsed = raw.parse::<u64>().ok()?;
    if parsed < min || parsed > max {
        return None;
    }
    Some(parsed)
}

fn parse_ranged_usize(raw: &str, min: usize, max: usize) -> Option<usize> {
    let parsed = raw.parse::<usize>().ok()?;
    if parsed < min || parsed > max {
        return None;
    }
    Some(parsed)
}

fn validate_voice_lab_value(spec: &VoiceLabFieldSpec, raw_value: &str) -> Option<String> {
    let trimmed = raw_value.trim();
    if trimmed.is_empty() {
        return if spec.default_value.is_empty() {
            Some(String::new())
        } else {
            None
        };
    }

    let valid = match spec.key {
        "CODESCRIBE_BUFFER_DELAY_MS" => parse_ranged_u64(trimmed, 0, 60_000).is_some(),
        "CODESCRIBE_TYPING_CPS" => parse_ranged_f32(trimmed, 5.0, 120.0).is_some(),
        "CODESCRIBE_EMIT_WORDS_MAX" => parse_ranged_usize(trimmed, 1, 12).is_some(),
        "CODESCRIBE_BUFFERED_INTERIM_SEC" => parse_ranged_f32(trimmed, 1.0, 30.0).is_some(),
        "BACKEND_MAX_UPLOAD_MB" => parse_ranged_u64(trimmed, 1, 200).is_some(),
        "WHISPER_MODEL" => true,
        _ => true,
    };

    if valid {
        Some(trimmed.to_string())
    } else {
        None
    }
}

fn voice_lab_spec_from_tag(tag: isize) -> Option<&'static VoiceLabFieldSpec> {
    if tag < 0 {
        return None;
    }
    VOICE_LAB_FIELDS.get(tag as usize)
}

#[derive(Default)]
struct BootstrapState {
    window: Option<usize>,
    window_delegate: Option<usize>,
    root_view: Option<usize>,
    step_labels: [Option<usize>; 3],
    tab_buttons: [Option<usize>; TAB_COUNT],
    content_views: [Option<usize>; TAB_COUNT],
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
    llm_key_status_label: Option<usize>,
    assistive_endpoint_field: Option<usize>,
    assistive_model_field: Option<usize>,
    assistive_key_field: Option<usize>,
    assistive_key_status_label: Option<usize>,
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

static SHOW_OVERLAY_IN_FLIGHT: AtomicBool = AtomicBool::new(false);

pub fn show_bootstrap_overlay() {
    // Fast path: if window already exists, just show it on main thread.
    {
        let state = BOOTSTRAP_STATE.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(ptr) = state.window {
            drop(state);
            Queue::main().exec_async(move || unsafe {
                let window = ptr as Id;
                window_show(window);
            });
            return;
        }
    }

    // Slow path: need to create window — guard against concurrent thread spawns.
    if SHOW_OVERLAY_IN_FLIGHT.swap(true, Ordering::SeqCst) {
        return;
    }
    std::thread::spawn(|| {
        let config = Config::load();
        Queue::main().exec_async(move || {
            SHOW_OVERLAY_IN_FLIGHT.store(false, Ordering::SeqCst);
            let mut state = BOOTSTRAP_STATE.lock().unwrap_or_else(|e| e.into_inner());
            state.config_cache = Some(config);
            drop(state);
            show_bootstrap_overlay_impl();
        });
    });
}

/// Alias: Settings window (bootstrap is now a standalone Settings window).
pub fn show_settings_window() {
    show_bootstrap_overlay();
}

fn show_bootstrap_overlay_impl() {
    // Keep Settings as a standalone window.
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
            warn!("No NSScreen available for settings window");
            return;
        }
        let visible: CGRect = msg_send![screen, visibleFrame];
        let window_width = SETTINGS_WINDOW_WIDTH;
        let window_height = SETTINGS_WINDOW_HEIGHT;
        let x = visible.origin.x + (visible.size.width - window_width) * 0.5;
        let y = visible.origin.y + (visible.size.height - window_height) * 0.5;
        let frame = CGRect::new(
            &CGPoint::new(x, y),
            &CGSize::new(window_width, window_height),
        );

        // Settings window should be fixed-size (no resize / fullscreen), to avoid AppKit
        // fullscreen transition crashes with our custom content setup.
        let window = create_floating_window(frame, "Settings", true, false);
        let _: () = msg_send![window, setOpaque: false];
        let _: () = msg_send![window, setLevel: crate::ui_helpers::NS_NORMAL_WINDOW_LEVEL];
        // Disallow fullscreen/zoom to avoid triggering AppKit fullscreen snapshots that can crash.
        let _: () =
            msg_send![window, setCollectionBehavior: NSWindowCollectionBehavior::FullScreenNone];
        // Hard lock the size (no resize handles, no zoom).
        let fixed_size = CGSize::new(window_width, window_height);
        let _: () = msg_send![window, setContentMinSize: fixed_size];
        let _: () = msg_send![window, setContentMaxSize: fixed_size];
        let zoom_btn: Id = msg_send![window, standardWindowButton: NSWindowButton::ZoomButton];
        if !zoom_btn.is_null() {
            let _: () = msg_send![zoom_btn, setEnabled: false];
        }
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

        // Create a container view (transparent) to hold the split visual effects.
        let ns_view = Class::get("NSView").unwrap();
        let root: Id = msg_send![ns_view, alloc];
        let root: Id = msg_send![root, initWithFrame: frame];
        let _: () = msg_send![
            root,
            setAutoresizingMask: 2_isize | 16_isize // NSViewWidthSizable | NSViewHeightSizable
        ];
        let _: () = msg_send![root, setWantsLayer: true];
        let root_layer: Id = msg_send![root, layer];
        if !root_layer.is_null() {
            let _: () = msg_send![root_layer, setCornerRadius: ui_tokens::CORNER_RADIUS_LG];
            let _: () = msg_send![root_layer, setMasksToBounds: true];
            let _: () = msg_send![root_layer, setBorderWidth: 0.0f64];
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

fn keychain_key_is_set(account: &str) -> bool {
    std::env::var(account)
        .ok()
        .map(|v| !v.trim().is_empty())
        .unwrap_or(false)
}

fn key_status_text(is_set: bool) -> &'static str {
    if is_set {
        "Stored in Keychain"
    } else {
        "Not set"
    }
}

fn key_status_color(is_set: bool) -> Id {
    unsafe {
        let ns_color = Class::get("NSColor").unwrap();
        if is_set {
            msg_send![ns_color, systemGreenColor]
        } else {
            msg_send![ns_color, secondaryLabelColor]
        }
    }
}

fn update_keychain_status_labels() {
    let (llm_label, assist_label) = {
        let state = BOOTSTRAP_STATE.lock().unwrap_or_else(|e| e.into_inner());
        (state.llm_key_status_label, state.assistive_key_status_label)
    };
    unsafe {
        if let Some(ptr) = llm_label {
            let is_set = keychain_key_is_set("LLM_API_KEY");
            let label = ptr as Id;
            set_text_field_string(label, key_status_text(is_set));
            let _: () = msg_send![label, setTextColor: key_status_color(is_set)];
        }
        if let Some(ptr) = assist_label {
            let is_set = keychain_key_is_set("LLM_ASSISTIVE_API_KEY");
            let label = ptr as Id;
            set_text_field_string(label, key_status_text(is_set));
            let _: () = msg_send![label, setTextColor: key_status_color(is_set)];
        }
    }
}

fn clear_keychain_entry(account: &str, field_ptr: Option<usize>) {
    if let Err(e) = keychain::delete_key(account) {
        warn!("Failed to delete {account} from Keychain: {e}");
    } else {
        info!("Deleted {account} from Keychain");
    }
    unsafe { std::env::remove_var(account) };
    if let Some(ptr) = field_ptr {
        unsafe { set_text_field_string(ptr as Id, "") };
    }
    update_keychain_status_labels();
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

        // ── Unified top toolbar (Vista-style: traffic spacer + center title + right actions) ──
        let topbar_h = SETTINGS_TOPBAR_HEIGHT
            .min(settings_height - 160.0)
            .max(44.0);
        let body_h = (settings_height - topbar_h).max(220.0);
        let topbar_frame = CGRect::new(
            &CGPoint::new(0.0, body_h),
            &CGSize::new(settings_width, topbar_h),
        );
        let topbar_bg = create_glass_effect_view_with(
            topbar_frame,
            NSVisualEffectMaterial::Titlebar,
            objc2_app_kit::NSVisualEffectBlendingMode::BehindWindow,
            objc2_app_kit::NSVisualEffectState::Active,
        );
        let _: () = msg_send![
            topbar_bg,
            setAutoresizingMask: 2_isize | 8_isize // Width | MinYMargin
        ];
        let topbar_layer: Id = msg_send![topbar_bg, layer];
        if !topbar_layer.is_null() {
            let _: () = msg_send![topbar_layer, setBorderWidth: 0.0f64];
        }
        add_subview(root_view, topbar_bg);

        let topbar_divider = create_label(LabelConfig {
            frame: CGRect::new(&CGPoint::new(0.0, 0.0), &CGSize::new(settings_width, 1.0)),
            text: String::new(),
            background_color: Some(ui_colors::separator()),
            ..Default::default()
        });
        let _: () = msg_send![topbar_divider, setAlphaValue: 0.44f64];
        let _: () = msg_send![
            topbar_divider,
            setAutoresizingMask: 2_isize | 8_isize // Width | MinYMargin
        ];
        add_subview(topbar_bg, topbar_divider);

        let topbar_controls: Id = msg_send![ns_view, alloc];
        let topbar_controls: Id = msg_send![
            topbar_controls,
            initWithFrame: CGRect::new(
                &CGPoint::new(0.0, 0.0),
                &CGSize::new(settings_width, topbar_h),
            )
        ];
        let _: () = msg_send![topbar_controls, setWantsLayer: true];
        let _: () = msg_send![
            topbar_controls,
            setAutoresizingMask: 2_isize | 16_isize // Width | Height
        ];
        add_subview(topbar_bg, topbar_controls);

        let btn_w = ui_tokens::HEADER_BUTTON_SIZE;
        let btn_h = ui_tokens::HEADER_BUTTON_SIZE;
        let right_pad = ui_tokens::EDGE_PADDING_TIGHT;
        let btn_y = ((topbar_h - btn_h) * 0.5).max(0.0);

        let overlay_btn_x = settings_width - right_pad - btn_w;
        let overlay_btn = create_button(
            CGRect::new(
                &CGPoint::new(overlay_btn_x, btn_y),
                &CGSize::new(btn_w, btn_h),
            ),
            "",
            button_style::INLINE,
        );
        let _ = set_button_symbol(overlay_btn, "bubble.left.and.bubble.right");
        style_toolbar_icon_button(overlay_btn);
        set_tooltip(overlay_btn, "Show chat overlay");
        button_set_action(overlay_btn, action_handler, sel!(onShowOverlay:));
        add_subview(topbar_controls, overlay_btn);

        let title_x = ui_tokens::TRAFFIC_LIGHTS_SPACER_WIDTH + 6.0;
        let title_label = create_label(LabelConfig {
            frame: CGRect::new(
                &CGPoint::new(title_x, topbar_h - 30.0),
                &CGSize::new(280.0, 20.0),
            ),
            text: "CodeScribe Settings".to_string(),
            font_size: 15.0,
            bold: true,
            text_color: crate::ui_helpers::color_label(),
            ..Default::default()
        });
        add_subview(topbar_controls, title_label);

        let subtitle_w = (overlay_btn_x - title_x - 14.0).max(200.0);
        let subtitle_label = create_label(LabelConfig {
            frame: CGRect::new(&CGPoint::new(title_x, 8.0), &CGSize::new(subtitle_w, 16.0)),
            text: "Native macOS speech-to-text setup and runtime tuning".to_string(),
            font_size: ui_tokens::SMALL_FONT_SIZE,
            bold: false,
            text_color: crate::ui_helpers::color_secondary_label(),
            ..Default::default()
        });
        add_subview(topbar_controls, subtitle_label);

        // ── Glass Split Structure ────────────────────────────────────
        // Left: Sidebar (Material: Sidebar)
        let sidebar_frame =
            CGRect::new(&CGPoint::new(0.0, 0.0), &CGSize::new(SIDEBAR_WIDTH, body_h));
        let sidebar_bg = create_glass_effect_view_with(
            sidebar_frame,
            NSVisualEffectMaterial::Sidebar,
            objc2_app_kit::NSVisualEffectBlendingMode::BehindWindow,
            objc2_app_kit::NSVisualEffectState::Active,
        );
        let _: () = msg_send![
            sidebar_bg,
            setAutoresizingMask: 16_isize | 2_isize // Height | MinXMargin (fixed left)
        ];
        add_subview(root_view, sidebar_bg);

        // Right: Content (Material: HUDWindow for richer contrast, aligned with onboarding)
        let content_bg_frame = CGRect::new(
            &CGPoint::new(SIDEBAR_WIDTH, 0.0),
            &CGSize::new(settings_width - SIDEBAR_WIDTH, body_h),
        );
        let content_bg = create_glass_effect_view_with(
            content_bg_frame,
            NSVisualEffectMaterial::HUDWindow,
            objc2_app_kit::NSVisualEffectBlendingMode::BehindWindow,
            objc2_app_kit::NSVisualEffectState::Active,
        );
        let _: () = msg_send![
            content_bg,
            setAutoresizingMask: 16_isize | 2_isize // Height | Width
        ];
        add_subview(root_view, content_bg);

        let split_divider = create_label(LabelConfig {
            frame: CGRect::new(
                &CGPoint::new(SIDEBAR_WIDTH - 0.5, 0.0),
                &CGSize::new(1.0, body_h),
            ),
            text: String::new(),
            background_color: Some(ui_colors::separator()),
            ..Default::default()
        });
        let _: () = msg_send![split_divider, setAlphaValue: 0.40f64];
        add_subview(root_view, split_divider);

        let content_area_w = content_bg_frame.size.width;
        let content_area_h = body_h;

        let sidebar_title = create_label(LabelConfig {
            frame: CGRect::new(
                &CGPoint::new(18.0, body_h - 34.0),
                &CGSize::new(SIDEBAR_WIDTH - 26.0, 20.0),
            ),
            text: "Settings".to_string(),
            font_size: ui_tokens::SMALL_FONT_SIZE,
            bold: true,
            text_color: crate::ui_helpers::color_label(),
            ..Default::default()
        });
        add_subview(sidebar_bg, sidebar_title);

        // Sidebar tab buttons (inside sidebar_bg)
        let tab_start_y = body_h - 86.0;
        let tab_names = ["Setup", "Keys", "Audio", "Voice Lab", "Engine"];
        let tab_sels = [
            sel!(onTabSetup:),
            sel!(onTabKeys:),
            sel!(onTabAudio:),
            sel!(onTabVoiceLab:),
            sel!(onTabEngine:),
        ];
        let mut tab_buttons: [Option<usize>; TAB_COUNT] = [None; TAB_COUNT];

        for (i, (name, sel)) in tab_names.iter().zip(tab_sels.iter()).enumerate() {
            let btn_height = 38.0;
            let gap = 6.0;
            let btn_y = tab_start_y - (btn_height + gap) * (i as f64);
            let btn_frame = CGRect::new(
                &CGPoint::new(10.0, btn_y),
                &CGSize::new(SIDEBAR_WIDTH - 20.0, btn_height),
            );

            let tab_btn = create_sidebar_tab_button(btn_frame, name, i == TAB_SETUP);
            button_set_action(tab_btn, action_handler, *sel);
            add_subview(sidebar_bg, tab_btn);
            tab_buttons[i] = Some(tab_btn as usize);
        }

        // ====================================================================
        // Content area views (one per tab, inside content_bg)
        // ====================================================================
        // Relative to content_bg: origin is (0,0)
        let tab_content_frame = CGRect::new(
            &CGPoint::new(SETTINGS_CONTENT_INSET_X, SETTINGS_CONTENT_INSET_Y),
            &CGSize::new(
                (content_area_w - SETTINGS_CONTENT_INSET_X * 2.0).max(240.0),
                (content_area_h - SETTINGS_CONTENT_INSET_Y * 2.0).max(220.0),
            ),
        );

        // --- Setup tab (index 0) ---
        let content_width = tab_content_frame.size.width;
        let content_h = tab_content_frame.size.height;

        let setup_view: Id = msg_send![ns_view, alloc];
        let setup_view: Id = msg_send![setup_view, initWithFrame: tab_content_frame];
        add_subview(content_bg, setup_view);

        let pad = ui_tokens::EDGE_PADDING;
        let field_w = content_width - pad * 2.0;
        let primary = crate::ui_helpers::color_label();
        let secondary = crate::ui_helpers::color_secondary_label();
        let mut y = content_h - 20.0;
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

        let llm_endpoint_val = config
            .llm_endpoint
            .clone()
            .unwrap_or_else(|| std::env::var("LLM_ENDPOINT").unwrap_or_default());
        let llm_endpoint_field = create_text_input(
            CGRect::new(&CGPoint::new(pad, y), &CGSize::new(field_w, 22.0)),
            "Endpoint (e.g. https://api.libraxis.cloud/v1/responses)",
            &llm_endpoint_val,
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
            "Model (e.g. programmer)",
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
        y -= 22.0;
        let llm_key_status = keychain_key_is_set("LLM_API_KEY");
        let llm_status_label = create_label(LabelConfig {
            frame: CGRect::new(&CGPoint::new(pad, y), &CGSize::new(field_w, 16.0)),
            text: key_status_text(llm_key_status).to_string(),
            font_size: ui_tokens::MICRO_FONT_SIZE,
            text_color: key_status_color(llm_key_status),
            ..Default::default()
        });
        add_subview(setup_view, llm_status_label);
        state.llm_key_status_label = Some(llm_status_label as usize);
        y -= 20.0;

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
            "Endpoint (e.g. https://api.libraxis.cloud/v1/responses)",
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
            "Model (e.g. programmer)",
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
        y -= 22.0;
        let assist_key_status = keychain_key_is_set("LLM_ASSISTIVE_API_KEY");
        let assist_status_label = create_label(LabelConfig {
            frame: CGRect::new(&CGPoint::new(pad, y), &CGSize::new(field_w, 16.0)),
            text: key_status_text(assist_key_status).to_string(),
            font_size: ui_tokens::MICRO_FONT_SIZE,
            text_color: key_status_color(assist_key_status),
            ..Default::default()
        });
        add_subview(setup_view, assist_status_label);
        state.assistive_key_status_label = Some(assist_status_label as usize);
        y -= 40.0;

        let save_btn = button(
            CGRect::new(
                &CGPoint::new(content_width - 110.0, y + 4.0),
                &CGSize::new(90.0, 24.0),
            ),
            "Save",
        );
        button_set_action(save_btn, action_handler, sel!(onSaveApiSettings:));
        add_subview(setup_view, save_btn);
        y -= 44.0;

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
        let completion: Id = msg_send![completion, initWithFrame: tab_content_frame];
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
        add_subview(content_bg, completion);

        // --- Keys tab (index 1) ---
        let keys_view = build_keys_tab(action_handler, tab_content_frame, config, &mut state);
        let _: () = msg_send![keys_view, setHidden: true];
        add_subview(content_bg, keys_view);

        // --- Audio tab (index 2) ---
        let audio_view = build_audio_tab(action_handler, tab_content_frame, config);
        let _: () = msg_send![audio_view, setHidden: true];
        add_subview(content_bg, audio_view);

        // --- Voice Lab tab (index 3) ---
        let voice_lab_view = build_voice_lab_tab(action_handler, tab_content_frame);
        let _: () = msg_send![voice_lab_view, setHidden: true];
        add_subview(content_bg, voice_lab_view);

        // --- Engine tab (index 4) ---
        let engine_view = build_engine_tab(tab_content_frame);
        let _: () = msg_send![engine_view, setHidden: true];
        add_subview(content_bg, engine_view);

        // ====================================================================
        // Store state
        // ====================================================================
        state.step_labels = step_status_labels;
        state.tab_buttons = tab_buttons;
        state.content_views = [
            Some(setup_view as usize),
            Some(keys_view as usize),
            Some(audio_view as usize),
            Some(voice_lab_view as usize),
            Some(engine_view as usize),
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
        // Left alignment for sidebar items
        let _: () = msg_send![btn, setAlignment: 0_isize]; // NSLeftTextAlignment

        // Add SF Symbol icon based on title
        let symbol_name = match title {
            "Setup" => "gearshape",
            "Keys" => "keyboard",
            "Audio" => "waveform",
            "Voice Lab" => "waveform.path.ecg",
            "Engine" => "cpu",
            _ => "circle",
        };
        crate::ui_helpers::set_button_symbol(btn, symbol_name);
        // NSImageLeft = 2
        let _: () = msg_send![btn, setImagePosition: 2_isize];

        let font: Id = msg_send![ns_font, systemFontOfSize: 13.5f64];
        let _: () = msg_send![btn, setFont: font];

        let _: () = msg_send![btn, setWantsLayer: true];
        let layer: Id = msg_send![btn, layer];
        if !layer.is_null() {
            let bg = if active {
                let ns_color = Class::get("NSColor").unwrap();
                let accent: Id = msg_send![ns_color, controlAccentColor];
                let semi: Id = msg_send![accent, colorWithAlphaComponent: 0.16f64];
                semi
            } else {
                crate::ui_helpers::color_clear()
            };
            let cg_color: Id = msg_send![bg, CGColor];
            let _: () = msg_send![layer, setBackgroundColor: cg_color];
            let _: () = msg_send![layer, setCornerRadius: 10.0f64];
            if active {
                let ns_color = Class::get("NSColor").unwrap();
                let accent: Id = msg_send![ns_color, controlAccentColor];
                let border: Id = msg_send![accent, colorWithAlphaComponent: 0.40f64];
                let cg_border: Id = msg_send![border, CGColor];
                let _: () = msg_send![layer, setBorderColor: cg_border];
                let _: () = msg_send![layer, setBorderWidth: 1.0f64];
            } else {
                let _: () = msg_send![layer, setBorderWidth: 0.0f64];
            }
        }

        let tint = if active {
            let ns_color = Class::get("NSColor").unwrap();
            msg_send![ns_color, controlAccentColor]
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
            if index >= TAB_COUNT || state.active_tab == index {
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
                    let bg: Id = if active {
                        let ns_color = Class::get("NSColor").unwrap();
                        let accent: Id = msg_send![ns_color, controlAccentColor];
                        msg_send![accent, colorWithAlphaComponent: 0.16f64]
                    } else {
                        crate::ui_helpers::color_clear()
                    };
                    let cg_color: Id = msg_send![bg, CGColor];
                    let _: () = msg_send![layer, setBackgroundColor: cg_color];
                    let _: () = msg_send![layer, setCornerRadius: 10.0f64];
                    if active {
                        let ns_color = Class::get("NSColor").unwrap();
                        let accent: Id = msg_send![ns_color, controlAccentColor];
                        let border: Id = msg_send![accent, colorWithAlphaComponent: 0.40f64];
                        let cg_border: Id = msg_send![border, CGColor];
                        let _: () = msg_send![layer, setBorderColor: cg_border];
                        let _: () = msg_send![layer, setBorderWidth: 1.0f64];
                    } else {
                        let _: () = msg_send![layer, setBorderWidth: 0.0f64];
                    }
                }

                let tint = if active {
                    let ns_color = Class::get("NSColor").unwrap();
                    let accent: Id = msg_send![ns_color, controlAccentColor];
                    accent
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
    state.tab_buttons = [None, None, None, None, None];
    state.content_views = [None, None, None, None, None];
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
    state.llm_key_status_label = None;
    state.assistive_endpoint_field = None;
    state.assistive_model_field = None;
    state.assistive_key_field = None;
    state.assistive_key_status_label = None;
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
                state.tab_buttons = [None, None, None, None, None];
                state.content_views = [None, None, None, None, None];
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
                state.llm_key_status_label = None;
                state.assistive_endpoint_field = None;
                state.assistive_model_field = None;
                state.assistive_key_field = None;
                state.assistive_key_status_label = None;
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

/// Alias: Settings window close.
pub fn hide_settings_window() {
    hide_bootstrap_overlay();
}

/// Alias: schedule Settings onboarding window.
pub fn schedule_settings_window() {
    schedule_bootstrap();
}

/// Alias: should show Settings onboarding window.
pub fn should_show_settings_onboarding() -> bool {
    should_show_bootstrap()
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
    state.tab_buttons = [None, None, None, None, None];
    state.content_views = [None, None, None, None, None];
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
    state.llm_key_status_label = None;
    state.assistive_endpoint_field = None;
    state.assistive_model_field = None;
    state.assistive_key_field = None;
    state.assistive_key_status_label = None;
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
            "Disabled (toggle only)",
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
            "none" => 5,
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

        // Formatting level dropdown
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
        // Pre-select based on current setting
        let current_level = std::env::var("FORMATTING_LEVEL").unwrap_or_default();
        let sel_idx: isize = match current_level.as_str() {
            "raw" => 0,
            "medium" => 1,
            "creative" => 2,
            _ => 1, // default to Medium
        };
        let _: () = msg_send![fmt_popup, selectItemAtIndex: sel_idx];
        button_set_action(fmt_popup, action_handler, sel!(onFormattingLevelChanged:));
        add_subview(container, fmt_popup);
        y -= 38.0;

        // Beep on start toggle
        let beep_check = create_checkbox(
            CGRect::new(&CGPoint::new(pad, y), &CGSize::new(content_w, 20.0)),
            "Beep on recording start",
            config.beep_on_start,
        );
        button_set_action(beep_check, action_handler, sel!(onBeepToggled:));
        add_subview(container, beep_check);
        y -= 34.0;
        // Agent: Enter to send toggle
        let enter_check = create_checkbox(
            CGRect::new(&CGPoint::new(pad, y), &CGSize::new(content_w, 20.0)),
            "Enter to send (⌘⏎ for newline)",
            config.agent_enter_sends,
        );
        button_set_action(enter_check, action_handler, sel!(onEnterSendToggled:));
        add_subview(container, enter_check);
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

unsafe fn build_voice_lab_tab(action_handler: Id, frame: core_graphics::geometry::CGRect) -> Id {
    use core_graphics::geometry::{CGPoint, CGRect, CGSize};
    unsafe {
        let ns_view = Class::get("NSView").unwrap();
        let ns_scroll_view = Class::get("NSScrollView").unwrap();
        let env_snapshot: HashMap<String, String> = std::env::vars().collect();

        let container: Id = msg_send![ns_view, alloc];
        let container: Id = msg_send![container, initWithFrame: frame];

        let pad = ui_tokens::EDGE_PADDING;
        let content_w = frame.size.width - pad * 2.0;
        let mut y = frame.size.height - 40.0;
        let primary = crate::ui_helpers::color_label();
        let secondary = crate::ui_helpers::color_secondary_label();

        let title = create_label(LabelConfig {
            frame: CGRect::new(&CGPoint::new(pad, y), &CGSize::new(content_w, 22.0)),
            text: "Voice Lab".to_string(),
            font_size: ui_tokens::BODY_FONT_SIZE,
            bold: true,
            text_color: primary,
            ..Default::default()
        });
        add_subview(container, title);
        y -= 20.0;

        let subtitle = create_label(LabelConfig {
            frame: CGRect::new(&CGPoint::new(pad, y), &CGSize::new(content_w, 16.0)),
            text: "Hot-reload transcription engine controls (persisted to config)".to_string(),
            font_size: ui_tokens::MICRO_FONT_SIZE,
            text_color: secondary,
            ..Default::default()
        });
        add_subview(container, subtitle);
        y -= 16.0;
        let apply_hint = create_label(LabelConfig {
            frame: CGRect::new(&CGPoint::new(pad, y), &CGSize::new(content_w, 14.0)),
            text: "Apply: press Enter or click outside the field.".to_string(),
            font_size: 10.0,
            text_color: secondary,
            ..Default::default()
        });
        add_subview(container, apply_hint);
        y -= 14.0;

        let scroll_h = (y - 18.0).max(160.0);
        let scroll_frame = CGRect::new(&CGPoint::new(pad, 12.0), &CGSize::new(content_w, scroll_h));
        let scroll: Id = msg_send![ns_scroll_view, alloc];
        let scroll: Id = msg_send![scroll, initWithFrame: scroll_frame];
        // Configure the vertical scroller after we know actual document height.
        let _: () = msg_send![scroll, setHasVerticalScroller: false];
        let _: () = msg_send![scroll, setHasHorizontalScroller: false];
        let _: () = msg_send![scroll, setAutohidesScrollers: true];
        let _: () = msg_send![scroll, setBorderType: 0_isize];
        let _: () = msg_send![scroll, setDrawsBackground: false];
        let _: () = msg_send![
            scroll,
            setAutoresizingMask: 2_isize | 16_isize // width + height sizable
        ];

        let mut doc_h: f64 = 18.0;
        for spec in VOICE_LAB_FIELDS {
            doc_h += if spec.kind == VoiceLabFieldKind::Bool {
                40.0
            } else {
                58.0
            };
        }
        doc_h = doc_h.max(18.0);
        let needs_vertical_scroll = doc_h > (scroll_h + 1.0);
        let _: () = msg_send![scroll, setHasVerticalScroller: needs_vertical_scroll];

        let doc_w = (content_w - 14.0).max(260.0);
        let doc_view: Id = msg_send![ns_view, alloc];
        let doc_view: Id = msg_send![doc_view, initWithFrame:
            CGRect::new(&CGPoint::new(0.0, 0.0), &CGSize::new(doc_w, doc_h))
        ];
        let _: () = msg_send![
            doc_view,
            setAutoresizingMask: 2_isize // width sizable
        ];

        let mut row_y = doc_h - 24.0;
        for (idx, spec) in VOICE_LAB_FIELDS.iter().enumerate() {
            match spec.kind {
                VoiceLabFieldKind::Bool => {
                    let checked =
                        parse_env_bool(&voice_lab_value_from_snapshot(spec, &env_snapshot));
                    let title = format!("{} ({})", spec.label, spec.key);
                    let check = create_checkbox(
                        CGRect::new(&CGPoint::new(0.0, row_y), &CGSize::new(doc_w - 6.0, 20.0)),
                        &title,
                        checked,
                    );
                    let _: () = msg_send![check, setTag: idx as isize];
                    button_set_action(check, action_handler, sel!(onVoiceLabToggleChanged:));
                    add_subview(doc_view, check);
                    row_y -= 18.0;

                    let desc = create_label(LabelConfig {
                        frame: CGRect::new(
                            &CGPoint::new(22.0, row_y),
                            &CGSize::new(doc_w - 24.0, 16.0),
                        ),
                        text: spec.description.to_string(),
                        font_size: ui_tokens::MICRO_FONT_SIZE,
                        text_color: secondary,
                        ..Default::default()
                    });
                    add_subview(doc_view, desc);
                    row_y -= 22.0;
                }
                VoiceLabFieldKind::Value => {
                    let label = create_label(LabelConfig {
                        frame: CGRect::new(&CGPoint::new(0.0, row_y), &CGSize::new(doc_w, 16.0)),
                        text: format!("{} ({})", spec.label, spec.key),
                        font_size: ui_tokens::MICRO_FONT_SIZE,
                        text_color: secondary,
                        ..Default::default()
                    });
                    add_subview(doc_view, label);
                    row_y -= 20.0;

                    let current = voice_lab_value_from_snapshot(spec, &env_snapshot);
                    let field = create_text_input(
                        CGRect::new(&CGPoint::new(0.0, row_y), &CGSize::new(doc_w - 6.0, 22.0)),
                        spec.default_value,
                        &current,
                    );
                    let _: () = msg_send![field, setTag: idx as isize];
                    button_set_action(field, action_handler, sel!(onVoiceLabFieldChanged:));
                    add_subview(doc_view, field);
                    row_y -= 20.0;

                    let desc = create_label(LabelConfig {
                        frame: CGRect::new(
                            &CGPoint::new(0.0, row_y),
                            &CGSize::new(doc_w - 8.0, 16.0),
                        ),
                        text: spec.description.to_string(),
                        font_size: ui_tokens::MICRO_FONT_SIZE,
                        text_color: secondary,
                        ..Default::default()
                    });
                    add_subview(doc_view, desc);
                    row_y -= 20.0;
                }
            }
        }

        let _: () = msg_send![scroll, setDocumentView: doc_view];
        add_subview(container, scroll);
        container
    }
}

// ============================================================================
// Engine tab — read-only engine status panel
// ============================================================================

unsafe fn build_engine_tab(frame: core_graphics::geometry::CGRect) -> Id {
    use core_graphics::geometry::{CGPoint, CGRect, CGSize};
    unsafe {
        let ns_view = Class::get("NSView").unwrap();
        let ns_color = Class::get("NSColor").unwrap();

        let container: Id = msg_send![ns_view, alloc];
        let container: Id = msg_send![container, initWithFrame: frame];

        let pad = ui_tokens::EDGE_PADDING;
        let content_w = frame.size.width - pad * 2.0;
        let mut y = frame.size.height - 40.0;
        let primary = crate::ui_helpers::color_label();
        let secondary = crate::ui_helpers::color_secondary_label();
        let mono = crate::ui_helpers::monospace_font(ui_tokens::SMALL_FONT_SIZE);

        // ── Title ──────────────────────────────────────────────
        let title = create_label(LabelConfig {
            frame: CGRect::new(&CGPoint::new(pad, y), &CGSize::new(content_w, 22.0)),
            text: "Engine".to_string(),
            font_size: ui_tokens::BODY_FONT_SIZE,
            bold: true,
            text_color: primary,
            ..Default::default()
        });
        add_subview(container, title);
        y -= 20.0;

        let subtitle = create_label(LabelConfig {
            frame: CGRect::new(&CGPoint::new(pad, y), &CGSize::new(content_w, 16.0)),
            text: "Runtime engine status (read-only)".to_string(),
            font_size: ui_tokens::MICRO_FONT_SIZE,
            text_color: secondary,
            ..Default::default()
        });
        add_subview(container, subtitle);
        y -= 28.0;

        // ── Helper: add a status row (dot + label + mono value) ─
        let mut add_row = |label_text: &str, value_text: &str, ok: bool| {
            let dot = if ok { "\u{25CF}" } else { "\u{25CB}" };
            let dot_color: Id = if ok {
                msg_send![ns_color, systemGreenColor]
            } else {
                msg_send![ns_color, systemOrangeColor]
            };

            let dot_lbl = create_label(LabelConfig {
                frame: CGRect::new(&CGPoint::new(pad, y), &CGSize::new(18.0, 18.0)),
                text: dot.to_string(),
                font_size: 14.0,
                text_color: dot_color,
                ..Default::default()
            });
            add_subview(container, dot_lbl);

            let lbl = create_label(LabelConfig {
                frame: CGRect::new(&CGPoint::new(pad + 20.0, y), &CGSize::new(120.0, 18.0)),
                text: label_text.to_string(),
                font_size: ui_tokens::SMALL_FONT_SIZE,
                bold: true,
                text_color: primary,
                ..Default::default()
            });
            add_subview(container, lbl);

            let val = create_label(LabelConfig {
                frame: CGRect::new(
                    &CGPoint::new(pad + 142.0, y),
                    &CGSize::new(content_w - 142.0, 18.0),
                ),
                text: value_text.to_string(),
                font_size: ui_tokens::SMALL_FONT_SIZE,
                text_color: secondary,
                ..Default::default()
            });
            let _: () = msg_send![val, setFont: mono];
            add_subview(container, val);
            y -= 28.0;
        };

        // ── STT Engine ─────────────────────────────────────────
        let stt_engine =
            std::env::var("CODESCRIBE_STT_ENGINE").unwrap_or_else(|_| "candle".to_string());
        let stt_label = match stt_engine.as_str() {
            "onnx" => "ONNX Runtime (Whisper)",
            _ => "Candle + Metal GPU",
        };
        add_row("STT Engine", stt_label, true);

        // ── Whisper Model ──────────────────────────────────────
        let whisper_embedded = codescribe_core::stt::whisper::embedded::is_embedded_available();
        let whisper_status = if whisper_embedded {
            "Embedded (~894 MB in binary)".to_string()
        } else {
            let path =
                std::env::var("CODESCRIBE_MODEL_PATH").unwrap_or_else(|_| "(not set)".to_string());
            let filename = path.rsplit('/').next().unwrap_or(&path);
            format!("External: {filename}")
        };
        add_row("Whisper", &whisper_status, whisper_embedded);

        // ── VAD (Silero) ───────────────────────────────────────
        let vad_embedded = codescribe_core::vad::embedded::is_embedded_available();
        let vad_status = if vad_embedded {
            "Silero v6 embedded (2.3 MB)".to_string()
        } else {
            let path = codescribe_core::vad::user_model_path();
            if path.exists() {
                let filename = path
                    .file_name()
                    .unwrap_or(path.as_os_str())
                    .to_string_lossy();
                format!("Silero v6: {filename}")
            } else {
                "Not found (will auto-download)".to_string()
            }
        };
        add_row(
            "VAD",
            &vad_status,
            vad_embedded || codescribe_core::vad::user_model_path().exists(),
        );

        // ── TTS Engine ─────────────────────────────────────────
        let tts_embedded = codescribe_core::tts::embedded::is_embedded_available();
        let tts_status = if tts_embedded {
            "CSM-1B embedded (~1 GB)"
        } else {
            "Not available"
        };
        add_row("TTS", tts_status, tts_embedded);

        // ── Embedder ───────────────────────────────────────────
        let embedder_ready = codescribe_core::embedder::is_initialized();
        add_row(
            "Embedder",
            if embedder_ready {
                "MiniLM ready"
            } else {
                "MiniLM (lazy init)"
            },
            true,
        );

        // ── Separator ──────────────────────────────────────────
        y -= 4.0;
        let sep = create_label(LabelConfig {
            frame: CGRect::new(&CGPoint::new(pad, y), &CGSize::new(content_w, 1.0)),
            text: String::new(),
            font_size: 1.0,
            ..Default::default()
        });
        let _: () = msg_send![sep, setWantsLayer: true];
        let layer: Id = msg_send![sep, layer];
        if !layer.is_null() {
            let bg: Id = msg_send![ns_color, separatorColor];
            let cg: Id = msg_send![bg, CGColor];
            let _: () = msg_send![layer, setBackgroundColor: cg];
        }
        add_subview(container, sep);
        y -= 20.0;

        // ── Env hint ───────────────────────────────────────────
        let hint = create_label(LabelConfig {
            frame: CGRect::new(&CGPoint::new(pad, y), &CGSize::new(content_w, 14.0)),
            text: "Switch STT: set CODESCRIBE_STT_ENGINE=onnx in .env".to_string(),
            font_size: 10.0,
            text_color: secondary,
            ..Default::default()
        });
        let _: () = msg_send![hint, setFont: mono];
        add_subview(container, hint);

        container
    }
}

// ============================================================================
// Settings handler stubs (Keys + Audio + Voice Lab tabs)
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
            5 => ("none", HoldMods::None),
            _ => ("fn", HoldMods::Fn),
        };
        info!("Settings: hold modifier -> {}", value);
        let config = Config::load();
        hotkeys::set_hold_mods(mods);

        // If DoubleCtrl toggle is enabled, Ctrl-only hold is unsafe → disable toggle.
        if mods == HoldMods::Ctrl && config.toggle_trigger == ToggleTrigger::DoubleCtrl {
            let _ = config.save_to_env_many(&[
                ("HOLD_MODS", value),
                ("TOGGLE_TRIGGER", ToggleTrigger::None.as_str()),
            ]);
            hotkeys::set_toggle_trigger(ToggleTrigger::None);

            let state = BOOTSTRAP_STATE.lock().unwrap_or_else(|e| e.into_inner());
            set_keys_popup_index(state.keys_toggle_popup, 0);
        } else {
            let _ = config.save_to_env("HOLD_MODS", value);
        }
        mark_keys_preset_custom();
        sync_runtime_config_via_ipc();
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
                let _ = config.save_to_env_many(&[
                    ("HOLD_MODS", HoldMods::Fn.as_str()),
                    ("TOGGLE_TRIGGER", ToggleTrigger::DoubleOption.as_str()),
                    ("HOLD_EXCLUSIVE", "0"),
                ]);
                hotkeys::set_hold_mods(HoldMods::Fn);
                hotkeys::set_toggle_trigger(ToggleTrigger::DoubleOption);
                hotkeys::set_exclusive_mode(false);

                let state = BOOTSTRAP_STATE.lock().unwrap_or_else(|e| e.into_inner());
                set_keys_popup_index(state.keys_hold_popup, 0);
                set_keys_popup_index(state.keys_toggle_popup, 4);
                set_keys_checkbox_state(state.keys_exclusive_checkbox, true);
                sync_runtime_config_via_ipc();
            }
            // Safe (no toggles)
            1 => {
                info!("Settings: hotkey preset -> safe");
                let config = Config::load();
                let _ = config.save_to_env_many(&[
                    ("HOLD_MODS", HoldMods::Fn.as_str()),
                    ("TOGGLE_TRIGGER", ToggleTrigger::None.as_str()),
                    ("HOLD_EXCLUSIVE", "1"),
                ]);
                hotkeys::set_hold_mods(HoldMods::Fn);
                hotkeys::set_toggle_trigger(ToggleTrigger::None);
                hotkeys::set_exclusive_mode(true);

                let state = BOOTSTRAP_STATE.lock().unwrap_or_else(|e| e.into_inner());
                set_keys_popup_index(state.keys_hold_popup, 0);
                set_keys_popup_index(state.keys_toggle_popup, 0);
                set_keys_checkbox_state(state.keys_exclusive_checkbox, false);
                sync_runtime_config_via_ipc();
            }
            _ => {
                info!("Settings: hotkey preset -> custom");
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
        mark_keys_preset_custom();
        sync_runtime_config_via_ipc();
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

        // If enabling DoubleCtrl and hold is Ctrl-only, switch to Ctrl+Option and enable modes.
        if trigger == ToggleTrigger::DoubleCtrl && config.hold_mods == HoldMods::Ctrl {
            let _ = config.save_to_env_many(&[
                ("HOLD_MODS", HoldMods::CtrlAlt.as_str()),
                ("HOLD_EXCLUSIVE", "0"),
            ]);
            hotkeys::set_hold_mods(HoldMods::CtrlAlt);
            hotkeys::set_exclusive_mode(false);

            let state = BOOTSTRAP_STATE.lock().unwrap_or_else(|e| e.into_inner());
            set_keys_popup_index(state.keys_hold_popup, 2);
            set_keys_checkbox_state(state.keys_exclusive_checkbox, true);
        }

        mark_keys_preset_custom();
        sync_runtime_config_via_ipc();
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

pub(super) extern "C" fn on_formatting_level_changed(
    _this: &Object,
    _cmd: objc::runtime::Sel,
    sender: Id,
) {
    unsafe {
        let idx: isize = msg_send![sender, indexOfSelectedItem];
        let level = match idx {
            0 => "raw",
            1 => "medium",
            2 => "creative",
            _ => "medium",
        };
        info!("Settings: Formatting level -> {}", level);
        let config = Config::load();
        let _ = config.save_to_env("FORMATTING_LEVEL", level);
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
            update_keychain_status_labels();
        }
    }
}

pub(super) extern "C" fn on_clear_llm_key(_this: &Object, _cmd: objc::runtime::Sel, _sender: Id) {
    let field_ptr = {
        let state = BOOTSTRAP_STATE.lock().unwrap_or_else(|e| e.into_inner());
        state.llm_key_field
    };
    clear_keychain_entry("LLM_API_KEY", field_ptr);
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

    let mut entries: Vec<(&str, String)> = Vec::new();
    unsafe {
        if let Some(ptr) = llm_endpoint {
            let value = crate::ui_helpers::get_text_field_string(ptr as Id);
            entries.push(("LLM_ENDPOINT", value.trim().to_string()));
        }
        if let Some(ptr) = llm_model {
            let value = crate::ui_helpers::get_text_field_string(ptr as Id);
            entries.push(("LLM_MODEL", value.trim().to_string()));
        }
        if let Some(ptr) = llm_key {
            let value = crate::ui_helpers::get_text_field_string(ptr as Id);
            if !value.trim().is_empty() {
                entries.push(("LLM_API_KEY", value.trim().to_string()));
            }
        }
        if let Some(ptr) = assist_endpoint {
            let value = crate::ui_helpers::get_text_field_string(ptr as Id);
            entries.push(("LLM_ASSISTIVE_ENDPOINT", value.trim().to_string()));
        }
        if let Some(ptr) = assist_model {
            let value = crate::ui_helpers::get_text_field_string(ptr as Id);
            entries.push(("LLM_ASSISTIVE_MODEL", value.trim().to_string()));
        }
        if let Some(ptr) = assist_key {
            let value = crate::ui_helpers::get_text_field_string(ptr as Id);
            if !value.trim().is_empty() {
                entries.push(("LLM_ASSISTIVE_API_KEY", value.trim().to_string()));
            }
        }
    }
    if !entries.is_empty() {
        let config = Config::load();
        let borrowed: Vec<(&str, &str)> = entries.iter().map(|(k, v)| (*k, v.as_str())).collect();
        let _ = config.save_to_env_many(&borrowed);
    }
    unsafe {
        if let Some(ptr) = llm_key {
            set_text_field_string(ptr as Id, "");
        }
        if let Some(ptr) = assist_key {
            set_text_field_string(ptr as Id, "");
        }
    }
    update_keychain_status_labels();
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
        sync_runtime_config_via_ipc();
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
        sync_runtime_config_via_ipc();
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

pub(super) extern "C" fn on_enter_send_toggled(
    _this: &Object,
    _cmd: objc::runtime::Sel,
    sender: Id,
) {
    unsafe {
        let state: isize = msg_send![sender, state];
        let enabled = state == 1;
        info!("Settings: agent enter sends -> {}", enabled);
        let config = Config::load();
        let _ = config.save_to_env("AGENT_ENTER_SENDS", if enabled { "1" } else { "0" });
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

pub(super) extern "C" fn on_voice_lab_toggle_changed(
    _this: &Object,
    _cmd: objc::runtime::Sel,
    sender: Id,
) {
    unsafe {
        let tag: isize = msg_send![sender, tag];
        let Some(spec) = voice_lab_spec_from_tag(tag) else {
            return;
        };
        if spec.kind != VoiceLabFieldKind::Bool {
            return;
        }
        let checked_state: isize = msg_send![sender, state];
        let enabled = checked_state == 1;
        info!("Settings: {} -> {}", spec.key, enabled);
        let config = Config::load();
        let _ = config.save_to_env(spec.key, if enabled { "1" } else { "0" });
    }
}

pub(super) extern "C" fn on_voice_lab_field_changed(
    _this: &Object,
    _cmd: objc::runtime::Sel,
    sender: Id,
) {
    unsafe {
        let tag: isize = msg_send![sender, tag];
        let Some(spec) = voice_lab_spec_from_tag(tag) else {
            return;
        };
        if spec.kind != VoiceLabFieldKind::Value {
            return;
        }

        let ns_val: Id = msg_send![sender, stringValue];
        let cstr: *const std::ffi::c_char = msg_send![ns_val, UTF8String];
        if cstr.is_null() {
            return;
        }
        let value = std::ffi::CStr::from_ptr(cstr)
            .to_string_lossy()
            .trim()
            .to_string();
        let Some(validated) = validate_voice_lab_value(spec, &value) else {
            warn!(
                "Settings: rejected invalid value for {} -> {:?}",
                spec.key, value
            );
            set_text_field_string(sender, &voice_lab_value(spec));
            return;
        };
        info!("Settings: {} -> {}", spec.key, validated);
        let config = Config::load();
        let _ = config.save_to_env(spec.key, &validated);
        set_text_field_string(sender, &validated);
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
            update_keychain_status_labels();
        }
    }
}

pub(super) extern "C" fn on_clear_assistive_key(
    _this: &Object,
    _cmd: objc::runtime::Sel,
    _sender: Id,
) {
    let field_ptr = {
        let state = BOOTSTRAP_STATE.lock().unwrap_or_else(|e| e.into_inner());
        state.assistive_key_field
    };
    clear_keychain_entry("LLM_ASSISTIVE_API_KEY", field_ptr);
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

fn sync_runtime_config_via_ipc() {
    if let Err(e) = send_ipc(IpcCommand::ReloadRuntimeConfig) {
        warn!("Settings: runtime config sync via IPC failed: {e}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serial_test::serial;

    #[test]
    fn voice_lab_validation_rejects_invalid_numeric() {
        let spec = VoiceLabFieldSpec {
            key: "CODESCRIBE_BUFFERED_INTERIM_SEC",
            label: "Interim cadence",
            default_value: "3.0",
            description: "",
            kind: VoiceLabFieldKind::Value,
        };
        assert!(validate_voice_lab_value(&spec, "abc").is_none());
        assert!(validate_voice_lab_value(&spec, "0.1").is_none());
        assert_eq!(
            validate_voice_lab_value(&spec, "4.5"),
            Some("4.5".to_string())
        );
    }

    #[test]
    fn voice_lab_validation_handles_empty_by_default_policy() {
        let non_empty = VoiceLabFieldSpec {
            key: "WHISPER_MODEL",
            label: "Whisper cloud model",
            default_value: "mlx-community/whisper-large-v3-mlx",
            description: "",
            kind: VoiceLabFieldKind::Value,
        };
        let allow_empty = VoiceLabFieldSpec {
            key: "CUSTOM_EMPTY_KEY",
            label: "Custom empty key",
            default_value: "",
            description: "",
            kind: VoiceLabFieldKind::Value,
        };
        assert!(validate_voice_lab_value(&non_empty, " ").is_none());
        assert_eq!(
            validate_voice_lab_value(&allow_empty, " "),
            Some(String::new())
        );
    }

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
