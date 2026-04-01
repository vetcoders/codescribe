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
use crate::os::permissions::PermissionStatus;
use crate::ui::bootstrap::handlers::{
    action_handler_class, toolbar_delegate_class, window_delegate_class,
};
use crate::ui::onboarding::{
    PERMISSION_ORDER, PermissionKind, open_permission_settings, permission_status,
    request_permission,
};
use crate::ui_helpers::{
    LabelConfig, add_subview, add_tafla_header_separator, button, button_set_action,
    create_card_view, create_checkbox, create_floating_window, create_label,
    create_secure_text_input, create_slider, create_tafla_split_shell, create_text_input,
    ns_string, set_text_field_string, set_tooltip, style_tafla_input, style_tafla_section,
    ui_colors, ui_tokens, window_close, window_content_view, window_show,
};

mod handlers;

// Type alias for Objective-C object pointers
type Id = *mut Object;

const SIDEBAR_WIDTH: f64 = 204.0;
const SETTINGS_WINDOW_WIDTH: f64 = 760.0;
const SETTINGS_WINDOW_HEIGHT: f64 = 660.0;
// Keep Settings readable while restoring stronger system glass.
const SETTINGS_MAX_OPACITY: f64 = ui_tokens::SETTINGS_WINDOW_OPACITY;
const SETTINGS_CONTENT_INSET_X: f64 = 20.0;
const SETTINGS_CONTENT_INSET_Y: f64 = 12.0;
const TAB_BUTTON_HEIGHT: f64 = 38.0;
const TAB_BUTTON_GAP: f64 = 6.0;
const TAB_ACTIVE_BG_ALPHA: f64 = 0.10;
const TAB_ACTIVE_BORDER_ALPHA: f64 = 0.22;
const SIDEBAR_INSET: f64 = 10.0;
const PERMISSION_ROW_HEIGHT: f64 = 24.0 + ui_tokens::DENSITY_COMFORTABLE;
const PERMISSION_BUTTON_WIDTH: f64 = 118.0;
const STEP_ROW_HEIGHT: f64 = 24.0 + ui_tokens::DENSITY_COMFORTABLE;
const SETUP_LAUNCH_PAD_BUTTON_HEIGHT: f64 = 28.0;
const CREATOR_CARD_HEIGHT: f64 = 132.0;
const CREATOR_CARD_GAP: f64 = 10.0;
const SETUP_TOP_OFFSET: f64 = 20.0;
const SETUP_POST_STEPS_GAP: f64 = 8.0;
const SETUP_SAVE_MIN_ANCHOR_Y: f64 = 52.0;
const SETUP_HINT_MIN_Y: f64 = 52.0;
const TAB_CREATOR: usize = 0;
const TAB_SETUP: usize = TAB_CREATOR;
const TAB_KEYS: usize = 1;
const TAB_AUDIO: usize = 2;
const TAB_VOICE_LAB: usize = 3;
const TAB_ENGINE: usize = 4;
pub(super) const TAB_USER: usize = 5;
const TAB_COUNT: usize = 6;

const TOGGLE_ROW_HEIGHT: f64 = 20.0;
const TOGGLE_ROW_LABEL_INDENT: f64 = 22.0;
const TOGGLE_ROW_DESC_OFFSET: f64 = 18.0;
const TOGGLE_ROW_DESC_HEIGHT: f64 = 16.0;
const SETTINGS_INPUT_HEIGHT: f64 = 22.0;

const STEP_TEST_MIC: usize = 0;
const STEP_SHOW_OVERLAY: usize = 1;
const STEP_PRESS_HOTKEY: usize = 2;

#[derive(Clone, Copy)]
struct ToggleRowSpec<'a> {
    title: &'a str,
    checked: bool,
    action: objc::runtime::Sel,
    description: Option<&'a str>,
    tag: Option<isize>,
    gap: f64,
}

unsafe fn intensify_settings_glass(view: Id) {
    let supports_emphasized: bool = msg_send![view, respondsToSelector: sel!(setEmphasized:)];
    if supports_emphasized {
        let _: () = msg_send![view, setEmphasized: true];
    }
}

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
        label: "Buffered flag (compat)",
        default_value: "1",
        description: "Deprecated compatibility flag; runtime pipeline remains event-based.",
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

fn toggle_row_step(has_description: bool, gap: f64) -> f64 {
    if has_description {
        TOGGLE_ROW_DESC_OFFSET + TOGGLE_ROW_DESC_HEIGHT + gap
    } else {
        TOGGLE_ROW_HEIGHT + gap
    }
}

fn setup_content_height(min_visible_height: f64, gap: f64) -> f64 {
    let permission_rows = PERMISSION_ORDER.len() as f64;
    let quick_start_steps = (STEP_PRESS_HOTKEY + 1) as f64;
    let flow_before_save = (22.0 + gap) // setup title
        + (1.0 + gap) // header separator
        + (16.0 + gap) // optional/non-blocking note
        + (20.0 + gap) // permissions header
        + permission_rows * PERMISSION_ROW_HEIGHT
        + gap // permissions divider
        + (20.0 + gap) // quick start header
        + quick_start_steps * STEP_ROW_HEIGHT
        + SETUP_POST_STEPS_GAP
        + (20.0 + gap) // launch pads header
        + (16.0 + gap) // launch pads hint
        + (SETUP_LAUNCH_PAD_BUTTON_HEIGHT + gap) * 2.0
        + (16.0 + gap) // checklist hint
        + gap // setup divider
        + (16.0 + gap); // tab routing hint
    min_visible_height.max((SETUP_TOP_OFFSET + flow_before_save + SETUP_SAVE_MIN_ANCHOR_Y).ceil())
}

unsafe fn autosize_tab_document_view(document_view: Id, minimum_height: f64) -> f64 {
    let subviews: Id = msg_send![document_view, subviews];
    if subviews.is_null() {
        let mut doc_frame: CGRect = msg_send![document_view, frame];
        doc_frame.origin = CGPoint::new(0.0, 0.0);
        doc_frame.size.height = minimum_height.max(doc_frame.size.height);
        let _: () = msg_send![document_view, setFrame: doc_frame];
        return doc_frame.size.height;
    }

    let count: usize = msg_send![subviews, count];
    if count == 0 {
        let mut doc_frame: CGRect = msg_send![document_view, frame];
        doc_frame.origin = CGPoint::new(0.0, 0.0);
        doc_frame.size.height = minimum_height.max(doc_frame.size.height);
        let _: () = msg_send![document_view, setFrame: doc_frame];
        return doc_frame.size.height;
    }

    let mut min_y = f64::INFINITY;
    let mut max_y = 0.0_f64;
    for idx in 0..count {
        let subview: Id = msg_send![subviews, objectAtIndex: idx];
        if subview.is_null() {
            continue;
        }
        let frame: CGRect = msg_send![subview, frame];
        min_y = min_y.min(frame.origin.y);
        max_y = max_y.max(frame.origin.y + frame.size.height);
    }

    let shift_y = if min_y.is_finite() && min_y < SETTINGS_CONTENT_INSET_Y {
        SETTINGS_CONTENT_INSET_Y - min_y
    } else {
        0.0
    };

    if shift_y > 0.0 {
        for idx in 0..count {
            let subview: Id = msg_send![subviews, objectAtIndex: idx];
            if subview.is_null() {
                continue;
            }
            let mut frame: CGRect = msg_send![subview, frame];
            frame.origin.y += shift_y;
            let _: () = msg_send![subview, setFrame: frame];
        }
        max_y += shift_y;
    }

    let mut doc_frame: CGRect = msg_send![document_view, frame];
    doc_frame.origin = CGPoint::new(0.0, 0.0);
    doc_frame.size.height = minimum_height.max(max_y.ceil());
    let _: () = msg_send![document_view, setFrame: doc_frame];
    doc_frame.size.height
}

unsafe fn wrap_tab_content_in_scroll_view(frame: CGRect, document_view: Id) -> Id {
    let ns_scroll_view = Class::get("NSScrollView").unwrap();
    let scroll: Id = msg_send![ns_scroll_view, alloc];
    let scroll: Id = msg_send![scroll, initWithFrame: frame];
    let _: () = msg_send![scroll, setHasVerticalScroller: true];
    let _: () = msg_send![scroll, setHasHorizontalScroller: false];
    let _: () = msg_send![scroll, setAutohidesScrollers: true];
    let _: () = msg_send![scroll, setBorderType: 0_isize]; // NSNoBorder
    let _: () = msg_send![scroll, setDrawsBackground: false];
    let _: () = msg_send![
        scroll,
        setAutoresizingMask: 2_isize | 16_isize // width + height
    ];

    let doc_h = unsafe { autosize_tab_document_view(document_view, frame.size.height) };
    let _: () = msg_send![scroll, setDocumentView: document_view];
    let _: () = msg_send![scroll, setHasVerticalScroller: doc_h > frame.size.height + 1.0];

    let clip_view: Id = msg_send![scroll, contentView];
    if !clip_view.is_null() {
        let top_point = CGPoint::new(0.0, (doc_h - frame.size.height).max(0.0));
        let _: () = msg_send![clip_view, scrollToPoint: top_point];
        let _: () = msg_send![scroll, reflectScrolledClipView: clip_view];
    }

    scroll
}

unsafe fn add_toggle_row(
    container: Id,
    action_handler: Id,
    x: f64,
    y: &mut f64,
    width: f64,
    secondary: Id,
    spec: ToggleRowSpec<'_>,
) -> Id {
    let toggle = create_checkbox(
        CGRect::new(&CGPoint::new(x, *y), &CGSize::new(width, TOGGLE_ROW_HEIGHT)),
        spec.title,
        spec.checked,
    );
    if let Some(tag) = spec.tag {
        let _: () = msg_send![toggle, setTag: tag];
    }
    unsafe {
        button_set_action(toggle, action_handler, spec.action);
        add_subview(container, toggle);
    }

    if let Some(desc) = spec.description {
        let desc_label = create_label(LabelConfig {
            frame: CGRect::new(
                &CGPoint::new(x + TOGGLE_ROW_LABEL_INDENT, *y - TOGGLE_ROW_DESC_OFFSET),
                &CGSize::new(
                    (width - TOGGLE_ROW_LABEL_INDENT).max(60.0),
                    TOGGLE_ROW_DESC_HEIGHT,
                ),
            ),
            text: desc.to_string(),
            font_size: ui_tokens::MICRO_FONT_SIZE,
            text_color: secondary,
            ..Default::default()
        });
        unsafe {
            add_subview(container, desc_label);
        }
    }

    *y -= toggle_row_step(spec.description.is_some(), spec.gap);
    toggle
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
    permission_labels: [Option<usize>; 5],
    permission_action_buttons: [Option<usize>; 5],
    permission_requested: [bool; 5],
    permission_polling: bool,
    finish_button: Option<usize>,
    quality_daemon_checkbox: Option<usize>,
    ultra_quality_checkbox: Option<usize>,
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

fn setup_done_path() -> PathBuf {
    Config::config_dir().join("setup_done")
}

fn onboarding_done_path() -> PathBuf {
    Config::config_dir().join("onboarding_done")
}

fn bootstrap_done_path() -> PathBuf {
    Config::config_dir().join("bootstrap_done")
}

fn migrate_legacy_setup_sentinel() {
    let setup_done = setup_done_path();
    if setup_done.exists() {
        return;
    }

    if onboarding_done_path().exists() && bootstrap_done_path().exists() {
        if let Some(parent) = setup_done.parent() {
            let _ = fs::create_dir_all(parent);
        }
        let _ = fs::write(setup_done, "done");
    }
}

pub fn should_show_setup() -> bool {
    migrate_legacy_setup_sentinel();
    !setup_done_path().exists()
}

pub fn should_show_bootstrap() -> bool {
    should_show_setup()
}

fn mark_setup_done() {
    let path = setup_done_path();
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    let _ = fs::write(path, "done");
}

pub fn schedule_bootstrap() {
    if !should_show_setup() {
        return;
    }

    thread::spawn(|| {
        thread::sleep(Duration::from_millis(800));
        show_creator_window();
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
                refresh_permission_indicators();
                start_permission_polling();
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

/// Primary graphical entrypoint for the native macOS Creator window.
pub fn show_creator_window() {
    show_settings_creator_tab();
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
            refresh_permission_indicators();
            start_permission_polling();
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
        let window = create_floating_window(frame, "CodeScribe Creator", false, false);
        // Keep Settings glass/opacity aligned with chat + transcription overlays.
        let _: () = msg_send![window, setAlphaValue: SETTINGS_MAX_OPACITY];
        let _: () = msg_send![window, setLevel: crate::ui_helpers::NS_NORMAL_WINDOW_LEVEL];
        let _: () = msg_send![window, setTitleVisibility: 0_isize]; // NSWindowTitleVisible
        let _: () = msg_send![window, setTitlebarAppearsTransparent: false];
        let _: () = msg_send![window, setTitle: ns_string("CodeScribe Creator")];
        let supports_subtitle: bool = msg_send![window, respondsToSelector: sel!(setSubtitle:)];
        if supports_subtitle {
            let _: () = msg_send![
                window,
                setSubtitle: ns_string("Native macOS creator, setup, and runtime tuning")
            ];
        }
        let toolbar_delegate_class = toolbar_delegate_class();
        let toolbar_delegate: Id = msg_send![toolbar_delegate_class, new];
        let ns_toolbar = Class::get("NSToolbar").unwrap();
        let toolbar: Id = msg_send![ns_toolbar, alloc];
        let toolbar: Id = msg_send![toolbar, initWithIdentifier: ns_string("settings-toolbar")];
        let _: () = msg_send![toolbar, setDelegate: toolbar_delegate];
        let _: () = msg_send![toolbar, setDisplayMode: 2_isize]; // NSToolbarDisplayModeIconOnly
        let _: () = msg_send![toolbar, setAllowsUserCustomization: false];
        let _: () = msg_send![toolbar, setAutosavesConfiguration: false];
        let _: () = msg_send![window, setToolbar: toolbar];
        let supports_toolbar_style: bool =
            msg_send![window, respondsToSelector: sel!(setToolbarStyle:)];
        if supports_toolbar_style {
            let _: () = msg_send![window, setToolbarStyle: 3_isize]; // NSWindowToolbarStyleUnified
        }
        let supports_toolbar_button: bool =
            msg_send![window, respondsToSelector: sel!(setShowsToolbarButton:)];
        if supports_toolbar_button {
            let _: () = msg_send![window, setShowsToolbarButton: false];
        }
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
        refresh_permission_indicators();
        start_permission_polling();
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
            refresh_permission_indicators();
            start_permission_polling();
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
        state.permission_action_buttons = built_state.permission_action_buttons;
        state.permission_requested = built_state.permission_requested;
        state.permission_polling = built_state.permission_polling;
        state.quality_daemon_checkbox = built_state.quality_daemon_checkbox;
        state.ultra_quality_checkbox = built_state.ultra_quality_checkbox;
        state.completion_view = built_state.completion_view;
        state.llm_endpoint_field = built_state.llm_endpoint_field;
        state.llm_model_field = built_state.llm_model_field;
        state.llm_key_field = built_state.llm_key_field;
        state.llm_key_status_label = built_state.llm_key_status_label;
        state.assistive_endpoint_field = built_state.assistive_endpoint_field;
        state.assistive_model_field = built_state.assistive_model_field;
        state.assistive_key_field = built_state.assistive_key_field;
        state.assistive_key_status_label = built_state.assistive_key_status_label;

        drop(state); // Release lock before permission calls to avoid deadlock.

        refresh_permission_indicators();
        start_permission_polling();
        Some(root)
    }
}

// ============================================================================
// Permission checks / setup readiness
// ============================================================================

fn permissions_all_granted() -> bool {
    PERMISSION_ORDER
        .iter()
        .all(|kind| permission_status(*kind) == PermissionStatus::Granted)
}

fn granted_permission_count() -> usize {
    PERMISSION_ORDER
        .iter()
        .filter(|kind| permission_status(**kind) == PermissionStatus::Granted)
        .count()
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct CreatorCardContent {
    title: String,
    subtitle: String,
    preview: String,
}

fn creator_binding_short_label(binding: crate::config::ShortcutBinding) -> &'static str {
    match binding {
        crate::config::ShortcutBinding::Disabled => "Off",
        crate::config::ShortcutBinding::HoldFn => "Hold Fn",
        crate::config::ShortcutBinding::HoldCtrl => "Hold Ctrl",
        crate::config::ShortcutBinding::HoldCtrlAlt => "Ctrl+Option",
        crate::config::ShortcutBinding::HoldCtrlShift => "Ctrl+Shift",
        crate::config::ShortcutBinding::HoldCtrlCmd => "Ctrl+Command",
        crate::config::ShortcutBinding::DoubleCtrl => "2x Ctrl",
        crate::config::ShortcutBinding::DoubleLeftOption => "2x L-Option",
        crate::config::ShortcutBinding::DoubleRightOption => "2x R-Option",
    }
}

fn creator_setup_card(
    setup_complete: bool,
    granted_permissions: usize,
    total_permissions: usize,
) -> CreatorCardContent {
    let subtitle = format!("{granted_permissions}/{total_permissions} permissions ready");
    if setup_complete && granted_permissions == total_permissions {
        CreatorCardContent {
            title: "Setup Complete".to_string(),
            subtitle,
            preview: "This Mac is ready. Use the launch pads below for daily tuning and tests."
                .to_string(),
        }
    } else if setup_complete {
        CreatorCardContent {
            title: "Permissions Drifted".to_string(),
            subtitle,
            preview: "Setup finished earlier, but one or more macOS permissions need attention."
                .to_string(),
        }
    } else if granted_permissions == total_permissions {
        CreatorCardContent {
            title: "Ready to Finish".to_string(),
            subtitle,
            preview: "Everything is granted. Press Finish Setup once to lock in this native shell."
                .to_string(),
        }
    } else {
        CreatorCardContent {
            title: "Setup in Progress".to_string(),
            subtitle,
            preview: "Grant the remaining permissions, then finish setup to stabilize the app."
                .to_string(),
        }
    }
}

fn creator_hotkey_card(settings: &crate::config::UserSettings) -> CreatorCardContent {
    let dictation = settings.mode_binding_for(crate::config::WorkMode::Dictation);
    let formatting = settings.mode_binding_for(crate::config::WorkMode::Formatting);
    let assistive = settings.mode_binding_for(crate::config::WorkMode::Assistive);

    CreatorCardContent {
        title: "Mode Bindings".to_string(),
        subtitle: format!("Dictation: {}", creator_binding_short_label(dictation)),
        preview: format!(
            "Formatting: {} | Assistive: {}",
            creator_binding_short_label(formatting),
            creator_binding_short_label(assistive)
        ),
    }
}

fn creator_runtime_card(
    config: &Config,
    quality_state: &crate::quality_loop::QualityDaemonState,
) -> CreatorCardContent {
    let live_preview = if config.use_local_stt {
        "Local Whisper owns live preview"
    } else {
        "Cloud STT is configured for capture"
    };
    let quality = if !quality_state.available {
        "offline".to_string()
    } else if quality_state.pending_mismatches == 0 {
        "OK".to_string()
    } else {
        format!("{} pending", quality_state.pending_mismatches)
    };

    CreatorCardContent {
        title: "Runtime Truth".to_string(),
        subtitle: live_preview.to_string(),
        preview: format!(
            "Formatting: {} | Quality: {} | Dock: {}",
            if config.ai_formatting_enabled {
                "on"
            } else {
                "off"
            },
            quality,
            if config.show_dock_icon {
                "shown"
            } else {
                "hidden"
            }
        ),
    }
}

fn permission_color(granted: bool) -> Id {
    if granted {
        ui_colors::status_granted()
    } else {
        ui_colors::status_denied()
    }
}

fn permission_row_label(kind: PermissionKind) -> &'static str {
    kind.title()
}

fn permission_action_title(
    kind: PermissionKind,
    status: PermissionStatus,
    requested: bool,
) -> Option<&'static str> {
    if status == PermissionStatus::Granted {
        None
    } else if kind == PermissionKind::FullDiskAccess || requested {
        Some("Open Settings")
    } else {
        Some("Grant")
    }
}

fn permission_kind_from_tag(tag: isize) -> Option<PermissionKind> {
    if tag < 0 {
        return None;
    }
    PERMISSION_ORDER.get(tag as usize).copied()
}

fn open_system_settings_security() {
    let _ = std::process::Command::new("open")
        .arg("x-apple.systempreferences:com.apple.preference.security")
        .spawn();
}

fn handle_permission_action(kind: PermissionKind) {
    let idx = kind.index();
    let already_requested = {
        let mut state = BOOTSTRAP_STATE.lock().unwrap_or_else(|e| e.into_inner());
        let was_requested = state.permission_requested[idx];
        state.permission_requested[idx] = true;
        was_requested
    };

    if kind == PermissionKind::FullDiskAccess || already_requested {
        open_permission_settings(kind);
        refresh_permission_indicators();
        return;
    }

    if kind == PermissionKind::Microphone {
        thread::spawn(move || {
            let _ = request_permission(kind);
            refresh_permission_indicators();
        });
        refresh_permission_indicators();
        return;
    }

    let granted = request_permission(kind);
    if !granted
        && matches!(
            kind,
            PermissionKind::Accessibility | PermissionKind::InputMonitoring
        )
    {
        open_permission_settings(kind);
    }

    refresh_permission_indicators();
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
    if is_set {
        ui_colors::status_granted()
    } else {
        ui_colors::secondary_label()
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

fn start_permission_polling() {
    let should_start = {
        let mut state = BOOTSTRAP_STATE.lock().unwrap_or_else(|e| e.into_inner());
        if state.permission_polling {
            false
        } else {
            state.permission_polling = true;
            true
        }
    };

    if !should_start {
        return;
    }

    thread::spawn(|| {
        loop {
            thread::sleep(Duration::from_secs(2));
            let keep_running = {
                let state = BOOTSTRAP_STATE.lock().unwrap_or_else(|e| e.into_inner());
                state.permission_polling
            };
            if !keep_running {
                break;
            }
            refresh_permission_indicators();
        }
    });
}

pub(super) fn refresh_permission_indicators() {
    Queue::main().exec_async(move || unsafe {
        let (labels, action_buttons, requested, finish_button) = {
            let state = BOOTSTRAP_STATE.lock().unwrap_or_else(|e| e.into_inner());
            (
                state.permission_labels,
                state.permission_action_buttons,
                state.permission_requested,
                state.finish_button,
            )
        };

        for kind in PERMISSION_ORDER {
            let idx = kind.index();
            let status = permission_status(kind);
            let granted = status == PermissionStatus::Granted;
            let marker = if granted { "\u{2713}" } else { "\u{2715}" };
            let text = format!("{marker} {}", permission_row_label(kind));

            if let Some(label_ptr) = labels[idx] {
                let label = label_ptr as Id;
                set_text_field_string(label, &text);
                let color = permission_color(granted);
                let _: () = msg_send![label, setTextColor: color];
            }

            if let Some(button_ptr) = action_buttons[idx] {
                let action_button = button_ptr as Id;
                if let Some(title) = permission_action_title(kind, status, requested[idx]) {
                    let _: () = msg_send![action_button, setHidden: false];
                    let _: () = msg_send![action_button, setTitle: ns_string(title)];
                } else {
                    let _: () = msg_send![action_button, setHidden: true];
                }
            }
        }

        if let Some(finish_ptr) = finish_button {
            let finish_button = finish_ptr as Id;
            let can_finish = !should_show_setup() || permissions_all_granted();
            let _: () = msg_send![finish_button, setEnabled: can_finish];
            set_tooltip(
                finish_button,
                if can_finish {
                    "Close Creator or complete setup."
                } else {
                    "Grant all required macOS permissions to enable Finish Setup."
                },
            );
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
        let mut state = BootstrapState::default();

        let settings_width = settings_width.max(SIDEBAR_WIDTH + 240.0);
        let settings_height = settings_height.max(280.0);
        let body_h = settings_height;

        let shell = create_tafla_split_shell(
            root_view,
            CGRect::new(
                &CGPoint::new(0.0, 0.0),
                &CGSize::new(settings_width, body_h),
            ),
            NSVisualEffectMaterial::FullScreenUI,
            SETTINGS_MAX_OPACITY,
            SIDEBAR_WIDTH,
        );
        intensify_settings_glass(shell.root_glass);

        let sidebar_container = shell.sidebar_container;
        let content_container = shell.content_container;
        let ns_view = Class::get("NSView").unwrap();

        let content_area_w = settings_width - SIDEBAR_WIDTH;
        let content_area_h = body_h;

        let sidebar_title = create_label(LabelConfig {
            frame: CGRect::new(
                &CGPoint::new(18.0, body_h - 34.0),
                &CGSize::new(SIDEBAR_WIDTH - 26.0, 20.0),
            ),
            text: "Creator".to_string(),
            font_size: ui_tokens::SMALL_FONT_SIZE,
            bold: true,
            text_color: crate::ui_helpers::color_label(),
            ..Default::default()
        });
        add_subview(sidebar_container, sidebar_title);

        // Sidebar tab buttons (inside sidebar container)
        let tab_start_y = body_h - 86.0;
        let tab_names = ["Creator", "Keys", "Audio", "Voice Lab", "Engine", "User"];
        let tab_sels = [
            sel!(onTabSetup:),
            sel!(onTabKeys:),
            sel!(onTabAudio:),
            sel!(onTabVoiceLab:),
            sel!(onTabEngine:),
            sel!(onTabUser:),
        ];
        let mut tab_buttons: [Option<usize>; TAB_COUNT] = [None; TAB_COUNT];

        for (i, (name, sel)) in tab_names.iter().zip(tab_sels.iter()).enumerate() {
            let btn_y = tab_start_y - (TAB_BUTTON_HEIGHT + TAB_BUTTON_GAP) * (i as f64);
            let btn_frame = CGRect::new(
                &CGPoint::new(SIDEBAR_INSET, btn_y),
                &CGSize::new(SIDEBAR_WIDTH - SIDEBAR_INSET * 2.0, TAB_BUTTON_HEIGHT),
            );

            let tab_btn = create_sidebar_tab_button(btn_frame, name, i == TAB_SETUP);
            button_set_action(tab_btn, action_handler, *sel);
            add_subview(sidebar_container, tab_btn);
            tab_buttons[i] = Some(tab_btn as usize);
        }

        // ====================================================================
        // Content area views (one per tab, inside content container)
        // ====================================================================
        // Relative to content container: origin is (0,0)
        let tab_content_frame = CGRect::new(
            &CGPoint::new(SETTINGS_CONTENT_INSET_X, SETTINGS_CONTENT_INSET_Y),
            &CGSize::new(
                (content_area_w - SETTINGS_CONTENT_INSET_X * 2.0).max(240.0),
                (content_area_h - SETTINGS_CONTENT_INSET_Y * 2.0).max(220.0),
            ),
        );

        // --- Setup tab (index 0) ---
        let content_width = tab_content_frame.size.width;
        let tab_document_frame = CGRect::new(
            &CGPoint::new(0.0, 0.0),
            &CGSize::new(tab_content_frame.size.width, tab_content_frame.size.height),
        );
        let setup_gap = ui_tokens::DENSITY_COMFORTABLE;
        let content_h = setup_content_height(tab_content_frame.size.height, setup_gap);
        let setup_document_frame = CGRect::new(
            &CGPoint::new(0.0, 0.0),
            &CGSize::new(content_width, content_h),
        );

        let setup_view: Id = msg_send![ns_view, alloc];
        let setup_view: Id = msg_send![setup_view, initWithFrame: setup_document_frame];
        style_tafla_section(setup_view);
        let setup_scroll = wrap_tab_content_in_scroll_view(tab_content_frame, setup_view);
        add_subview(content_container, setup_scroll);

        let pad = ui_tokens::EDGE_PADDING;
        let field_w = content_width - pad * 2.0;
        let primary = crate::ui_helpers::color_label();
        let secondary = crate::ui_helpers::color_secondary_label();
        let mut y = content_h - SETUP_TOP_OFFSET;
        let setup_complete = !should_show_setup();
        let granted_permissions = granted_permission_count();
        let total_permissions = PERMISSION_ORDER.len();
        let setup_card = creator_setup_card(setup_complete, granted_permissions, total_permissions);
        let hotkey_card = creator_hotkey_card(&crate::config::UserSettings::load());
        let runtime_card = creator_runtime_card(config, &crate::quality_loop::read_daemon_state());

        // ── Permissions ───────────────────────────────────────────────
        let mut perm_labels: [Option<usize>; 5] = [None; 5];
        let mut perm_action_buttons: [Option<usize>; 5] = [None; 5];

        let setup_title = create_label(LabelConfig {
            frame: CGRect::new(&CGPoint::new(pad, y), &CGSize::new(field_w, 22.0)),
            text: "Creator Studio".to_string(),
            font_size: ui_tokens::BODY_FONT_SIZE,
            bold: true,
            text_color: primary,
            ..Default::default()
        });
        add_subview(setup_view, setup_title);
        y -= 22.0 + setup_gap;

        y = add_tafla_header_separator(setup_view, pad, y, field_w);
        y -= setup_gap;

        let setup_hint_top = create_label(LabelConfig {
            frame: CGRect::new(&CGPoint::new(pad, y), &CGSize::new(field_w, 16.0)),
            text: "Native launchpad for first-run setup, testing, and daily runtime tuning."
                .to_string(),
            font_size: ui_tokens::MICRO_FONT_SIZE,
            text_color: secondary,
            ..Default::default()
        });
        add_subview(setup_view, setup_hint_top);
        y -= 16.0 + setup_gap;

        let card_width = ((field_w - CREATOR_CARD_GAP * 2.0) / 3.0).max(150.0);
        for (index, card) in [setup_card, hotkey_card, runtime_card]
            .into_iter()
            .enumerate()
        {
            let x = pad + index as f64 * (card_width + CREATOR_CARD_GAP);
            let card_view = create_card_view(
                CGRect::new(
                    &CGPoint::new(x, y - CREATOR_CARD_HEIGHT + 8.0),
                    &CGSize::new(card_width, CREATOR_CARD_HEIGHT),
                ),
                &card.title,
                &card.subtitle,
                &card.preview,
            );
            add_subview(setup_view, card_view);
        }
        y -= CREATOR_CARD_HEIGHT + setup_gap;

        let permissions_header = create_label(LabelConfig {
            frame: CGRect::new(&CGPoint::new(pad, y), &CGSize::new(field_w, 20.0)),
            text: format!("Permissions Checklist ({granted_permissions}/{total_permissions})"),
            font_size: ui_tokens::SMALL_FONT_SIZE,
            bold: true,
            text_color: primary,
            ..Default::default()
        });
        add_subview(setup_view, permissions_header);
        y -= 20.0 + setup_gap;

        let permission_button_w = PERMISSION_BUTTON_WIDTH;
        let permission_label_w = (field_w - permission_button_w - 12.0).max(180.0);

        for kind in PERMISSION_ORDER {
            let idx = kind.index();
            let status = permission_status(kind);
            let granted = status == PermissionStatus::Granted;
            let marker = if granted { "\u{2713}" } else { "\u{2715}" };
            let text = format!("{marker} {}", permission_row_label(kind));

            let label = create_label(LabelConfig {
                frame: CGRect::new(
                    &CGPoint::new(pad, y),
                    &CGSize::new(permission_label_w, 20.0),
                ),
                text,
                font_size: ui_tokens::BODY_FONT_SIZE,
                bold: true,
                text_color: permission_color(granted),
                ..Default::default()
            });
            add_subview(setup_view, label);
            perm_labels[idx] = Some(label as usize);

            let initial_button_title =
                permission_action_title(kind, status, false).unwrap_or("Grant");
            let action_btn = button(
                CGRect::new(
                    &CGPoint::new(content_width - pad - permission_button_w, y - 2.0),
                    &CGSize::new(permission_button_w, 24.0),
                ),
                initial_button_title,
            );
            let _: () = msg_send![action_btn, setTag: idx as isize];
            button_set_action(action_btn, action_handler, sel!(onPermissionAction:));
            if permission_action_title(kind, status, false).is_none() {
                let _: () = msg_send![action_btn, setHidden: true];
            }
            add_subview(setup_view, action_btn);
            perm_action_buttons[idx] = Some(action_btn as usize);
            y -= PERMISSION_ROW_HEIGHT;
        }

        let permissions_divider = create_label(LabelConfig {
            frame: CGRect::new(&CGPoint::new(pad, y), &CGSize::new(field_w, 1.0)),
            text: String::new(),
            background_color: Some(ui_colors::surface_border()),
            ..Default::default()
        });
        let _: () = msg_send![permissions_divider, setAlphaValue: 0.9f64];
        add_subview(setup_view, permissions_divider);
        y -= setup_gap;

        let quick_start_header = create_label(LabelConfig {
            frame: CGRect::new(&CGPoint::new(pad, y), &CGSize::new(field_w, 20.0)),
            text: "Quick Start".to_string(),
            font_size: ui_tokens::SMALL_FONT_SIZE,
            bold: true,
            text_color: primary,
            ..Default::default()
        });
        add_subview(setup_view, quick_start_header);
        y -= 20.0 + setup_gap;

        // ── Quick-start steps ────────────────────────────────────────
        let step_defs: [(&str, objc::runtime::Sel, &str); 3] = [
            ("1. Test microphone", sel!(onTestMic:), "Test"),
            ("2. Open agent overlay", sel!(onShowOverlay:), "Show"),
            ("3. Try your hotkey", sel!(onHotkeyDone:), "Done"),
        ];
        let mut step_status_labels: [Option<usize>; 3] = [None; 3];

        for (i, (label_text, sel, btn_text)) in step_defs.iter().enumerate() {
            let step_label = create_label(LabelConfig {
                frame: CGRect::new(&CGPoint::new(pad, y), &CGSize::new(240.0, 20.0)),
                text: label_text.to_string(),
                font_size: ui_tokens::BODY_FONT_SIZE,
                bold: true,
                text_color: primary,
                ..Default::default()
            });
            add_subview(setup_view, step_label);

            let status_lbl = create_label(LabelConfig {
                frame: CGRect::new(&CGPoint::new(pad + 250.0, y), &CGSize::new(120.0, 20.0)),
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
            y -= STEP_ROW_HEIGHT;
        }
        y -= SETUP_POST_STEPS_GAP;

        let launch_header = create_label(LabelConfig {
            frame: CGRect::new(&CGPoint::new(pad, y), &CGSize::new(field_w, 20.0)),
            text: "Launch Pads".to_string(),
            font_size: ui_tokens::SMALL_FONT_SIZE,
            bold: true,
            text_color: primary,
            ..Default::default()
        });
        add_subview(setup_view, launch_header);
        y -= 20.0 + setup_gap;

        let launch_hint = create_label(LabelConfig {
            frame: CGRect::new(&CGPoint::new(pad, y), &CGSize::new(field_w, 16.0)),
            text: "Jump straight to the native surface you need right now.".to_string(),
            font_size: ui_tokens::MICRO_FONT_SIZE,
            text_color: secondary,
            ..Default::default()
        });
        add_subview(setup_view, launch_hint);
        y -= 16.0 + setup_gap;

        let launch_button_w = ((field_w - setup_gap) / 2.0).max(150.0);
        let right_button_x = pad + field_w - launch_button_w;

        let open_voice_lab_button = button(
            CGRect::new(
                &CGPoint::new(pad, y - 2.0),
                &CGSize::new(launch_button_w, SETUP_LAUNCH_PAD_BUTTON_HEIGHT),
            ),
            "Open Voice Lab",
        );
        button_set_action(open_voice_lab_button, action_handler, sel!(onTabVoiceLab:));
        add_subview(setup_view, open_voice_lab_button);

        let open_keys_button = button(
            CGRect::new(
                &CGPoint::new(right_button_x, y - 2.0),
                &CGSize::new(launch_button_w, SETUP_LAUNCH_PAD_BUTTON_HEIGHT),
            ),
            "Open Keys",
        );
        button_set_action(open_keys_button, action_handler, sel!(onTabKeys:));
        add_subview(setup_view, open_keys_button);
        y -= SETUP_LAUNCH_PAD_BUTTON_HEIGHT + setup_gap;

        let open_audio_button = button(
            CGRect::new(
                &CGPoint::new(pad, y - 2.0),
                &CGSize::new(launch_button_w, SETUP_LAUNCH_PAD_BUTTON_HEIGHT),
            ),
            "Open Audio",
        );
        button_set_action(open_audio_button, action_handler, sel!(onTabAudio:));
        add_subview(setup_view, open_audio_button);

        let open_engine_button = button(
            CGRect::new(
                &CGPoint::new(right_button_x, y - 2.0),
                &CGSize::new(launch_button_w, SETUP_LAUNCH_PAD_BUTTON_HEIGHT),
            ),
            "Open Engine",
        );
        button_set_action(open_engine_button, action_handler, sel!(onTabEngine:));
        add_subview(setup_view, open_engine_button);
        y -= SETUP_LAUNCH_PAD_BUTTON_HEIGHT + setup_gap;

        let open_user_button = button(
            CGRect::new(
                &CGPoint::new(pad, y - 2.0),
                &CGSize::new(launch_button_w, SETUP_LAUNCH_PAD_BUTTON_HEIGHT),
            ),
            "Open User",
        );
        button_set_action(open_user_button, action_handler, sel!(onTabUser:));
        add_subview(setup_view, open_user_button);

        let show_agent_button = button(
            CGRect::new(
                &CGPoint::new(right_button_x, y - 2.0),
                &CGSize::new(launch_button_w, SETUP_LAUNCH_PAD_BUTTON_HEIGHT),
            ),
            "Show Agent",
        );
        button_set_action(show_agent_button, action_handler, sel!(onShowOverlay:));
        add_subview(setup_view, show_agent_button);
        y -= SETUP_LAUNCH_PAD_BUTTON_HEIGHT + setup_gap;

        let checklist_hint = create_label(LabelConfig {
            frame: CGRect::new(&CGPoint::new(pad, y), &CGSize::new(field_w, 16.0)),
            text: "Use Creator as the native launchpad for setup, diagnostics, and daily tuning."
                .to_string(),
            font_size: ui_tokens::MICRO_FONT_SIZE,
            text_color: secondary,
            ..Default::default()
        });
        add_subview(setup_view, checklist_hint);
        y -= 16.0 + setup_gap;

        let setup_divider = create_label(LabelConfig {
            frame: CGRect::new(&CGPoint::new(pad, y), &CGSize::new(field_w, 1.0)),
            text: String::new(),
            background_color: Some(ui_colors::surface_border()),
            ..Default::default()
        });
        let _: () = msg_send![setup_divider, setAlphaValue: 0.9f64];
        add_subview(setup_view, setup_divider);
        y -= setup_gap;

        let setup_hint = create_label(LabelConfig {
            frame: CGRect::new(
                &CGPoint::new(pad, (y - 6.0).max(SETUP_HINT_MIN_Y)),
                &CGSize::new(field_w, 16.0),
            ),
            text:
                "Keys stores providers, Audio controls capture, Voice Lab tunes the live pipeline."
                    .to_string(),
            font_size: ui_tokens::MICRO_FONT_SIZE,
            text_color: secondary,
            ..Default::default()
        });
        add_subview(setup_view, setup_hint);

        // ── Footer buttons ───────────────────────────────────────────
        let finish_btn = button(
            CGRect::new(
                &CGPoint::new(content_width - 110.0, 16.0),
                &CGSize::new(90.0, 28.0),
            ),
            if setup_complete {
                "Close"
            } else {
                "Finish Setup"
            },
        );
        button_set_action(finish_btn, action_handler, sel!(onFinish:));
        let finish_enabled = setup_complete || granted_permissions == total_permissions;
        let _: () = msg_send![finish_btn, setEnabled: finish_enabled];
        set_tooltip(
            finish_btn,
            if finish_enabled {
                "Close Creator or complete setup."
            } else {
                "Grant all required macOS permissions to enable Finish Setup."
            },
        );
        add_subview(setup_view, finish_btn);

        let helper_btn = button(
            CGRect::new(&CGPoint::new(pad, 16.0), &CGSize::new(90.0, 28.0)),
            "Show Agent",
        );
        button_set_action(helper_btn, action_handler, sel!(onShowOverlay:));
        add_subview(setup_view, helper_btn);

        // ── Completion view (hidden, shown on Finish) ────────────────
        let completion: Id = msg_send![ns_view, alloc];
        let completion: Id = msg_send![completion, initWithFrame: tab_content_frame];
        let _: () = msg_send![completion, setHidden: true];
        let done_label = create_label(LabelConfig {
            frame: CGRect::new(
                &CGPoint::new(0.0, tab_content_frame.size.height * 0.5 - 20.0),
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
        add_subview(content_container, completion);

        // --- Keys tab (index 1) ---
        let keys_view = build_keys_tab(action_handler, tab_document_frame, config, &mut state);
        let keys_scroll = wrap_tab_content_in_scroll_view(tab_content_frame, keys_view);
        let _: () = msg_send![keys_scroll, setHidden: true];
        add_subview(content_container, keys_scroll);

        // --- Audio tab (index 2) ---
        let audio_view = build_audio_tab(action_handler, tab_document_frame, config);
        let audio_scroll = wrap_tab_content_in_scroll_view(tab_content_frame, audio_view);
        let _: () = msg_send![audio_scroll, setHidden: true];
        add_subview(content_container, audio_scroll);

        // --- Voice Lab tab (index 3) ---
        let voice_lab_view = build_voice_lab_tab(action_handler, tab_document_frame);
        let voice_lab_scroll = wrap_tab_content_in_scroll_view(tab_content_frame, voice_lab_view);
        let _: () = msg_send![voice_lab_scroll, setHidden: true];
        add_subview(content_container, voice_lab_scroll);

        // --- Engine tab (index 4) ---
        let engine_view = build_engine_tab(tab_document_frame);
        let engine_scroll = wrap_tab_content_in_scroll_view(tab_content_frame, engine_view);
        let _: () = msg_send![engine_scroll, setHidden: true];
        add_subview(content_container, engine_scroll);

        // --- User tab (index 5) ---
        let user_view = build_user_tab(action_handler, tab_document_frame, config, &mut state);
        let user_scroll = wrap_tab_content_in_scroll_view(tab_content_frame, user_view);
        let _: () = msg_send![user_scroll, setHidden: true];
        add_subview(content_container, user_scroll);

        // ====================================================================
        // Store state
        // ====================================================================
        state.step_labels = step_status_labels;
        state.tab_buttons = tab_buttons;
        state.content_views = [
            Some(setup_scroll as usize),
            Some(keys_scroll as usize),
            Some(audio_scroll as usize),
            Some(voice_lab_scroll as usize),
            Some(engine_scroll as usize),
            Some(user_scroll as usize),
        ];
        state.active_tab = TAB_SETUP;
        state.permission_labels = perm_labels;
        state.permission_action_buttons = perm_action_buttons;
        state.permission_requested = [false; 5];
        state.permission_polling = false;
        state.finish_button = Some(finish_btn as usize);
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
            "Creator" => "sparkles",
            "Keys" => "keyboard",
            "Audio" => "waveform",
            "Voice Lab" => "waveform.path.ecg",
            "Engine" => "cpu",
            "User" => "person.crop.circle",
            _ => "circle",
        };
        crate::ui_helpers::set_button_symbol(btn, symbol_name);
        // NSImageLeft = 2
        let _: () = msg_send![btn, setImagePosition: 2_isize];

        let font: Id = msg_send![ns_font, systemFontOfSize: ui_tokens::BODY_FONT_SIZE];
        let _: () = msg_send![btn, setFont: font];

        let _: () = msg_send![btn, setWantsLayer: true];
        let layer: Id = msg_send![btn, layer];
        if !layer.is_null() {
            let bg = if active {
                ui_colors::accent_tint(TAB_ACTIVE_BG_ALPHA)
            } else {
                crate::ui_helpers::color_clear()
            };
            let cg_color: Id = msg_send![bg, CGColor];
            let _: () = msg_send![layer, setBackgroundColor: cg_color];
            let _: () = msg_send![layer, setCornerRadius: ui_tokens::SURFACE_RADIUS];
            if active {
                let border = ui_colors::accent_tint(TAB_ACTIVE_BORDER_ALPHA);
                let cg_border: Id = msg_send![border, CGColor];
                let _: () = msg_send![layer, setBorderColor: cg_border];
                let _: () = msg_send![layer, setBorderWidth: 1.0f64];
            } else {
                let _: () = msg_send![layer, setBorderWidth: 0.0f64];
            }
        }

        let tint = if active {
            ui_colors::accent()
        } else {
            ui_colors::secondary_label()
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
                        ui_colors::accent_tint(TAB_ACTIVE_BG_ALPHA)
                    } else {
                        crate::ui_helpers::color_clear()
                    };
                    let cg_color: Id = msg_send![bg, CGColor];
                    let _: () = msg_send![layer, setBackgroundColor: cg_color];
                    let _: () = msg_send![layer, setCornerRadius: ui_tokens::SURFACE_RADIUS];
                    if active {
                        let border: Id = ui_colors::accent_tint(TAB_ACTIVE_BORDER_ALPHA);
                        let cg_border: Id = msg_send![border, CGColor];
                        let _: () = msg_send![layer, setBorderColor: cg_border];
                        let _: () = msg_send![layer, setBorderWidth: 1.0f64];
                    } else {
                        let _: () = msg_send![layer, setBorderWidth: 0.0f64];
                    }
                }

                let tint = if active {
                    ui_colors::accent()
                } else {
                    ui_colors::secondary_label()
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
    if !should_show_setup() {
        hide_bootstrap_overlay();
        return;
    }

    if !permissions_all_granted() {
        refresh_permission_indicators();
        warn!("Finish Setup requested before all permissions were granted; keeping Creator open.");
        return;
    }

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
        if permissions_all_granted() {
            mark_setup_done();
        } else {
            warn!(
                "Setup finish requested but not all permissions are granted yet; keeping setup incomplete."
            );
        }
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
    state.tab_buttons = [None; TAB_COUNT];
    state.content_views = [None; TAB_COUNT];
    state.keys_hold_popup = None;
    state.keys_toggle_popup = None;
    state.keys_preset_popup = None;
    state.keys_exclusive_checkbox = None;
    state.hold_delay_value_label = None;
    state.double_tap_value_label = None;
    state.permission_labels = [None, None, None, None, None];
    state.permission_action_buttons = [None, None, None, None, None];
    state.permission_requested = [false; 5];
    state.permission_polling = false;
    state.finish_button = None;
    state.quality_daemon_checkbox = None;
    state.ultra_quality_checkbox = None;
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
            state.permission_polling = false;
            let window_ptr = state.window.take();
            if window_ptr.is_some() {
                state.window_delegate = None;
                state.root_view = None;
                state.step_labels = [None, None, None];
                state.tab_buttons = [None; TAB_COUNT];
                state.content_views = [None; TAB_COUNT];
                state.keys_hold_popup = None;
                state.keys_toggle_popup = None;
                state.keys_preset_popup = None;
                state.keys_exclusive_checkbox = None;
                state.hold_delay_value_label = None;
                state.double_tap_value_label = None;
                state.permission_labels = [None, None, None, None, None];
                state.permission_action_buttons = [None, None, None, None, None];
                state.permission_requested = [false; 5];
                state.permission_polling = false;
                state.finish_button = None;
                state.quality_daemon_checkbox = None;
                state.ultra_quality_checkbox = None;
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

/// Show Settings and force-focus the Setup tab.
pub fn show_settings_setup_tab() {
    show_bootstrap_overlay();
    switch_tab(TAB_CREATOR);
}

/// Show the native Creator tab explicitly.
pub fn show_settings_creator_tab() {
    show_bootstrap_overlay();
    switch_tab(TAB_CREATOR);
}

/// Alias: should show Settings onboarding window.
pub fn should_show_settings_onboarding() -> bool {
    should_show_setup()
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
    state.tab_buttons = [None; TAB_COUNT];
    state.content_views = [None; TAB_COUNT];
    state.keys_hold_popup = None;
    state.keys_toggle_popup = None;
    state.keys_preset_popup = None;
    state.keys_exclusive_checkbox = None;
    state.hold_delay_value_label = None;
    state.double_tap_value_label = None;
    state.permission_labels = [None, None, None, None, None];
    state.permission_action_buttons = [None, None, None, None, None];
    state.permission_requested = [false; 5];
    state.permission_polling = false;
    state.finish_button = None;
    state.quality_daemon_checkbox = None;
    state.ultra_quality_checkbox = None;
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
        style_tafla_section(container);

        let pad = ui_tokens::EDGE_PADDING;
        let content_w = frame.size.width - pad * 2.0;
        let gap = ui_tokens::DENSITY_MEDIUM;
        let mut y = frame.size.height - (22.0 + gap);
        let primary = crate::ui_helpers::color_label();
        let secondary = crate::ui_helpers::color_secondary_label();
        let mono_font_input = crate::ui_helpers::monospace_font(ui_tokens::BODY_FONT_SIZE);

        // Section title
        let title = create_label(LabelConfig {
            frame: CGRect::new(&CGPoint::new(pad, y), &CGSize::new(content_w, 22.0)),
            text: "Keys & Configuration".to_string(),
            font_size: ui_tokens::BODY_FONT_SIZE,
            bold: true,
            text_color: primary,
            ..Default::default()
        });
        add_subview(container, title);
        y -= 22.0 + gap;

        y = add_tafla_header_separator(container, pad, y, content_w);
        y -= gap;

        let subtitle = create_label(LabelConfig {
            frame: CGRect::new(&CGPoint::new(pad, y), &CGSize::new(content_w, 16.0)),
            text: "Hotkeys plus all API/runtime key configuration.".to_string(),
            font_size: ui_tokens::MICRO_FONT_SIZE,
            text_color: secondary,
            ..Default::default()
        });
        add_subview(container, subtitle);
        y -= 16.0 + gap;

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
        y -= 24.0 + gap;

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
        y -= 24.0 + gap;

        // Shift/Cmd modes toggle
        let modes_check = add_toggle_row(
            container,
            action_handler,
            pad,
            &mut y,
            content_w,
            secondary,
            ToggleRowSpec {
                title: "Enable Shift/Cmd modes (Chat/Selection)",
                checked: !config.hold_exclusive,
                action: sel!(onHoldExclusiveChanged:),
                description: None,
                tag: None,
                gap,
            },
        );

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
        y -= 24.0 + gap;

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
        y -= 20.0 + gap;

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

        y -= 20.0 + gap;

        let config_divider = create_label(LabelConfig {
            frame: CGRect::new(&CGPoint::new(pad, y), &CGSize::new(content_w, 1.0)),
            text: String::new(),
            background_color: Some(ui_colors::surface_border()),
            ..Default::default()
        });
        let _: () = msg_send![config_divider, setAlphaValue: 0.9f64];
        add_subview(container, config_divider);
        y -= gap;

        let runtime_header = create_label(LabelConfig {
            frame: CGRect::new(&CGPoint::new(pad, y), &CGSize::new(content_w, 18.0)),
            text: "AI Runtime Configuration".to_string(),
            font_size: ui_tokens::SMALL_FONT_SIZE,
            bold: true,
            text_color: primary,
            ..Default::default()
        });
        add_subview(container, runtime_header);
        y -= 18.0 + gap;

        let runtime_subtitle = create_label(LabelConfig {
            frame: CGRect::new(&CGPoint::new(pad, y), &CGSize::new(content_w, 16.0)),
            text: "Formatting + Assistive models and endpoints.".to_string(),
            font_size: ui_tokens::MICRO_FONT_SIZE,
            text_color: secondary,
            ..Default::default()
        });
        add_subview(container, runtime_subtitle);
        y -= 16.0 + gap;

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

        let llm_endpoint_val = config
            .llm_endpoint
            .clone()
            .unwrap_or_else(|| std::env::var("LLM_ENDPOINT").unwrap_or_default());
        let llm_endpoint_field = create_text_input(
            CGRect::new(
                &CGPoint::new(pad, y),
                &CGSize::new(content_w, SETTINGS_INPUT_HEIGHT),
            ),
            "Endpoint (e.g. https://api.libraxis.cloud/v1/responses)",
            &llm_endpoint_val,
        );
        style_tafla_input(llm_endpoint_field);
        let _: () = msg_send![llm_endpoint_field, setFont: mono_font_input];
        button_set_action(
            llm_endpoint_field,
            action_handler,
            sel!(onLlmEndpointChanged:),
        );
        add_subview(container, llm_endpoint_field);
        state.llm_endpoint_field = Some(llm_endpoint_field as usize);
        y -= SETTINGS_INPUT_HEIGHT + gap;

        let llm_model_val = std::env::var("LLM_MODEL").unwrap_or_default();
        let llm_model_field = create_text_input(
            CGRect::new(
                &CGPoint::new(pad, y),
                &CGSize::new(content_w, SETTINGS_INPUT_HEIGHT),
            ),
            "Model (e.g. programmer)",
            &llm_model_val,
        );
        style_tafla_input(llm_model_field);
        let _: () = msg_send![llm_model_field, setFont: mono_font_input];
        button_set_action(llm_model_field, action_handler, sel!(onLlmModelChanged:));
        add_subview(container, llm_model_field);
        state.llm_model_field = Some(llm_model_field as usize);
        y -= SETTINGS_INPUT_HEIGHT + gap;

        let llm_key_field = create_secure_text_input(
            CGRect::new(
                &CGPoint::new(pad, y),
                &CGSize::new(content_w, SETTINGS_INPUT_HEIGHT),
            ),
            "API Key (stored in Keychain)",
        );
        style_tafla_input(llm_key_field);
        let _: () = msg_send![llm_key_field, setFont: mono_font_input];
        button_set_action(llm_key_field, action_handler, sel!(onLlmKeyChanged:));
        add_subview(container, llm_key_field);
        state.llm_key_field = Some(llm_key_field as usize);
        y -= SETTINGS_INPUT_HEIGHT + gap;

        let llm_key_status = keychain_key_is_set("LLM_API_KEY");
        let llm_status_label = create_label(LabelConfig {
            frame: CGRect::new(&CGPoint::new(pad, y), &CGSize::new(content_w, 16.0)),
            text: key_status_text(llm_key_status).to_string(),
            font_size: ui_tokens::MICRO_FONT_SIZE,
            text_color: key_status_color(llm_key_status),
            ..Default::default()
        });
        add_subview(container, llm_status_label);
        state.llm_key_status_label = Some(llm_status_label as usize);
        y -= 16.0 + gap;

        let assist_header = create_label(LabelConfig {
            frame: CGRect::new(&CGPoint::new(pad, y), &CGSize::new(content_w, 18.0)),
            text: "Assistive AI (optional)".to_string(),
            font_size: ui_tokens::SMALL_FONT_SIZE,
            bold: true,
            text_color: secondary,
            ..Default::default()
        });
        add_subview(container, assist_header);
        y -= 18.0 + gap;

        let assist_endpoint_val = std::env::var("LLM_ASSISTIVE_ENDPOINT").unwrap_or_default();
        let assist_endpoint_field = create_text_input(
            CGRect::new(
                &CGPoint::new(pad, y),
                &CGSize::new(content_w, SETTINGS_INPUT_HEIGHT),
            ),
            "Endpoint (e.g. https://api.libraxis.cloud/v1/responses)",
            &assist_endpoint_val,
        );
        style_tafla_input(assist_endpoint_field);
        let _: () = msg_send![assist_endpoint_field, setFont: mono_font_input];
        button_set_action(
            assist_endpoint_field,
            action_handler,
            sel!(onAssistiveEndpointChanged:),
        );
        add_subview(container, assist_endpoint_field);
        state.assistive_endpoint_field = Some(assist_endpoint_field as usize);
        y -= SETTINGS_INPUT_HEIGHT + gap;

        let assist_model_val = std::env::var("LLM_ASSISTIVE_MODEL").unwrap_or_default();
        let assist_model_field = create_text_input(
            CGRect::new(
                &CGPoint::new(pad, y),
                &CGSize::new(content_w, SETTINGS_INPUT_HEIGHT),
            ),
            "Model (e.g. programmer)",
            &assist_model_val,
        );
        style_tafla_input(assist_model_field);
        let _: () = msg_send![assist_model_field, setFont: mono_font_input];
        button_set_action(
            assist_model_field,
            action_handler,
            sel!(onAssistiveModelChanged:),
        );
        add_subview(container, assist_model_field);
        state.assistive_model_field = Some(assist_model_field as usize);
        y -= SETTINGS_INPUT_HEIGHT + gap;

        let assist_key_field = create_secure_text_input(
            CGRect::new(
                &CGPoint::new(pad, y),
                &CGSize::new(content_w, SETTINGS_INPUT_HEIGHT),
            ),
            "API Key (stored in Keychain)",
        );
        style_tafla_input(assist_key_field);
        let _: () = msg_send![assist_key_field, setFont: mono_font_input];
        button_set_action(
            assist_key_field,
            action_handler,
            sel!(onAssistiveKeyChanged:),
        );
        add_subview(container, assist_key_field);
        state.assistive_key_field = Some(assist_key_field as usize);
        y -= SETTINGS_INPUT_HEIGHT + gap;

        let assist_key_status = keychain_key_is_set("LLM_ASSISTIVE_API_KEY");
        let assist_status_label = create_label(LabelConfig {
            frame: CGRect::new(&CGPoint::new(pad, y), &CGSize::new(content_w, 16.0)),
            text: key_status_text(assist_key_status).to_string(),
            font_size: ui_tokens::MICRO_FONT_SIZE,
            text_color: key_status_color(assist_key_status),
            ..Default::default()
        });
        add_subview(container, assist_status_label);
        state.assistive_key_status_label = Some(assist_status_label as usize);
        y -= 16.0 + gap;

        let save_btn = button(
            CGRect::new(
                &CGPoint::new(frame.size.width - pad - 90.0, y - 2.0),
                &CGSize::new(90.0, 24.0),
            ),
            "Save",
        );
        button_set_action(save_btn, action_handler, sel!(onSaveApiSettings:));
        add_subview(container, save_btn);

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
        style_tafla_section(container);

        let pad = ui_tokens::EDGE_PADDING;
        let content_w = frame.size.width - pad * 2.0;
        let gap = ui_tokens::DENSITY_MEDIUM;
        let mut y = frame.size.height - (22.0 + gap);
        let primary = crate::ui_helpers::color_label();
        let secondary = crate::ui_helpers::color_secondary_label();

        // Section title
        let title = create_label(LabelConfig {
            frame: CGRect::new(&CGPoint::new(pad, y), &CGSize::new(content_w, 22.0)),
            text: "Speech & Interaction".to_string(),
            font_size: ui_tokens::BODY_FONT_SIZE,
            bold: true,
            text_color: primary,
            ..Default::default()
        });
        add_subview(container, title);
        y -= 22.0 + gap;

        y = add_tafla_header_separator(container, pad, y, content_w);
        y -= gap;

        let subtitle = create_label(LabelConfig {
            frame: CGRect::new(&CGPoint::new(pad, y), &CGSize::new(content_w, 16.0)),
            text: "Speech capture, formatting, and interaction behavior.".to_string(),
            font_size: ui_tokens::MICRO_FONT_SIZE,
            text_color: secondary,
            ..Default::default()
        });
        add_subview(container, subtitle);
        y -= 16.0 + gap;

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
        y -= 24.0 + gap;

        // AI Formatting toggle
        let _fmt_check = add_toggle_row(
            container,
            action_handler,
            pad,
            &mut y,
            content_w,
            secondary,
            ToggleRowSpec {
                title: "AI Formatting",
                checked: config.ai_formatting_enabled,
                action: sel!(onFormattingToggled:),
                description: Some("Use LLM to clean up transcriptions"),
                tag: None,
                gap,
            },
        );

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
        y -= 24.0 + gap;

        // Beep on start toggle
        let _beep_check = add_toggle_row(
            container,
            action_handler,
            pad,
            &mut y,
            content_w,
            secondary,
            ToggleRowSpec {
                title: "Beep on recording start",
                checked: config.beep_on_start,
                action: sel!(onBeepToggled:),
                description: None,
                tag: None,
                gap,
            },
        );
        // Agent: Enter to send toggle
        let _enter_check = add_toggle_row(
            container,
            action_handler,
            pad,
            &mut y,
            content_w,
            secondary,
            ToggleRowSpec {
                title: "Enter to send (⌘⏎ for newline)",
                checked: config.agent_enter_sends,
                action: sel!(onEnterSendToggled:),
                description: None,
                tag: None,
                gap,
            },
        );
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
        style_tafla_section(container);

        let pad = ui_tokens::EDGE_PADDING;
        let content_w = frame.size.width - pad * 2.0;
        let gap = ui_tokens::DENSITY_COMPACT;
        let mut y = frame.size.height - (22.0 + gap);
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
        y -= 22.0 + gap;

        y = add_tafla_header_separator(container, pad, y, content_w);
        y -= gap;

        let subtitle = create_label(LabelConfig {
            frame: CGRect::new(&CGPoint::new(pad, y), &CGSize::new(content_w, 16.0)),
            text: "Hot-reload transcription engine controls (persisted to config)".to_string(),
            font_size: ui_tokens::MICRO_FONT_SIZE,
            text_color: secondary,
            ..Default::default()
        });
        add_subview(container, subtitle);
        y -= 16.0 + gap;
        let apply_hint = create_label(LabelConfig {
            frame: CGRect::new(&CGPoint::new(pad, y), &CGSize::new(content_w, 14.0)),
            text: "Apply: press Enter or click outside the field.".to_string(),
            font_size: 10.0,
            text_color: secondary,
            ..Default::default()
        });
        add_subview(container, apply_hint);
        y -= 14.0 + gap;

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
                toggle_row_step(true, gap)
            } else {
                16.0 + gap + SETTINGS_INPUT_HEIGHT + gap + 16.0 + gap
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
                    let _check = add_toggle_row(
                        doc_view,
                        action_handler,
                        0.0,
                        &mut row_y,
                        doc_w - 6.0,
                        secondary,
                        ToggleRowSpec {
                            title: &title,
                            checked,
                            action: sel!(onVoiceLabToggleChanged:),
                            description: Some(spec.description),
                            tag: Some(idx as isize),
                            gap,
                        },
                    );
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
                    row_y -= 16.0 + gap;

                    let current = voice_lab_value_from_snapshot(spec, &env_snapshot);
                    let field = create_text_input(
                        CGRect::new(
                            &CGPoint::new(0.0, row_y),
                            &CGSize::new(doc_w - 6.0, SETTINGS_INPUT_HEIGHT),
                        ),
                        spec.default_value,
                        &current,
                    );
                    style_tafla_input(field);
                    let _: () = msg_send![field, setTag: idx as isize];
                    button_set_action(field, action_handler, sel!(onVoiceLabFieldChanged:));
                    add_subview(doc_view, field);
                    row_y -= SETTINGS_INPUT_HEIGHT + gap;

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
                    row_y -= 16.0 + gap;
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

        let container: Id = msg_send![ns_view, alloc];
        let container: Id = msg_send![container, initWithFrame: frame];
        style_tafla_section(container);

        let pad = ui_tokens::EDGE_PADDING;
        let content_w = frame.size.width - pad * 2.0;
        let gap = ui_tokens::DENSITY_COMPACT;
        let mut y = frame.size.height - (22.0 + gap);
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
        y -= 22.0 + gap;

        y = add_tafla_header_separator(container, pad, y, content_w);
        y -= gap;

        let subtitle = create_label(LabelConfig {
            frame: CGRect::new(&CGPoint::new(pad, y), &CGSize::new(content_w, 16.0)),
            text: "Runtime engine status (read-only)".to_string(),
            font_size: ui_tokens::MICRO_FONT_SIZE,
            text_color: secondary,
            ..Default::default()
        });
        add_subview(container, subtitle);
        y -= 16.0 + gap;

        // ── Helper: add a status row (dot + label + mono value) ─
        let mut add_row = |label_text: &str, value_text: &str, ok: bool| {
            let dot = if ok { "\u{25CF}" } else { "\u{25CB}" };
            let dot_color: Id = if ok {
                ui_colors::status_granted()
            } else {
                ui_colors::status_warning()
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
            y -= 18.0 + gap;
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
            let bg = ui_colors::surface_border();
            let cg: Id = msg_send![bg, CGColor];
            let _: () = msg_send![layer, setBackgroundColor: cg];
        }
        add_subview(container, sep);
        y -= gap;

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

unsafe fn build_user_tab(
    action_handler: Id,
    frame: core_graphics::geometry::CGRect,
    config: &Config,
    state: &mut BootstrapState,
) -> Id {
    use core_graphics::geometry::{CGPoint, CGRect, CGSize};
    unsafe {
        let ns_view = Class::get("NSView").unwrap();
        let container: Id = msg_send![ns_view, alloc];
        let container: Id = msg_send![container, initWithFrame: frame];
        style_tafla_section(container);

        let pad = ui_tokens::EDGE_PADDING;
        let content_w = frame.size.width - pad * 2.0;
        let field_w = content_w;
        let gap = ui_tokens::DENSITY_COMFORTABLE;
        let mut y = frame.size.height - (22.0 + gap);
        let primary = crate::ui_helpers::color_label();
        let secondary = crate::ui_helpers::color_secondary_label();

        let title = create_label(LabelConfig {
            frame: CGRect::new(&CGPoint::new(pad, y), &CGSize::new(content_w, 22.0)),
            text: "User".to_string(),
            font_size: ui_tokens::BODY_FONT_SIZE,
            bold: true,
            text_color: primary,
            ..Default::default()
        });
        add_subview(container, title);
        y -= 22.0 + gap;

        y = add_tafla_header_separator(container, pad, y, content_w);
        y -= gap;

        let subtitle = create_label(LabelConfig {
            frame: CGRect::new(&CGPoint::new(pad, y), &CGSize::new(content_w, 16.0)),
            text: "Customization and quality controls".to_string(),
            font_size: ui_tokens::MICRO_FONT_SIZE,
            text_color: secondary,
            ..Default::default()
        });
        add_subview(container, subtitle);
        y -= 16.0 + gap;

        let app_header = create_label(LabelConfig {
            frame: CGRect::new(&CGPoint::new(pad, y), &CGSize::new(content_w, 18.0)),
            text: "App".to_string(),
            font_size: ui_tokens::SMALL_FONT_SIZE,
            bold: true,
            text_color: primary,
            ..Default::default()
        });
        add_subview(container, app_header);
        y -= 18.0 + gap;

        let _dock_check = add_toggle_row(
            container,
            action_handler,
            pad,
            &mut y,
            field_w,
            secondary,
            ToggleRowSpec {
                title: "Show dock icon",
                checked: config.show_dock_icon,
                action: sel!(onShowDockIconToggled:),
                description: Some(
                    "Best effort at runtime; some launch modes keep current behavior.",
                ),
                tag: None,
                gap,
            },
        );

        let app_divider = create_label(LabelConfig {
            frame: CGRect::new(&CGPoint::new(pad, y), &CGSize::new(field_w, 1.0)),
            text: String::new(),
            background_color: Some(ui_colors::surface_border()),
            ..Default::default()
        });
        let _: () = msg_send![app_divider, setAlphaValue: 0.9f64];
        add_subview(container, app_divider);
        y -= gap;

        let quality_header = create_label(LabelConfig {
            frame: CGRect::new(&CGPoint::new(pad, y), &CGSize::new(content_w, 18.0)),
            text: "Transcription Quality".to_string(),
            font_size: ui_tokens::SMALL_FONT_SIZE,
            bold: true,
            text_color: primary,
            ..Default::default()
        });
        add_subview(container, quality_header);
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
                title: "Ultra Quality (slow final pass, explicit opt-in)",
                checked: ultra_on,
                action: sel!(onUltraQualityToggled:),
                description: Some("Runs legacy end-of-session pass for max quality (slower)."),
                tag: None,
                gap,
            },
        );
        state.ultra_quality_checkbox = Some(ultra_check as usize);

        let quality_on = std::env::var("CODESCRIBE_AUTOSTART_QUALITY_DAEMON")
            .map(|v| parse_env_bool(&v))
            .unwrap_or(false);
        let quality_check = add_toggle_row(
            container,
            action_handler,
            pad,
            &mut y,
            field_w,
            secondary,
            ToggleRowSpec {
                title: "Auto-tune transcription quality (recommended)",
                checked: quality_on,
                action: sel!(onQualityDaemonToggled:),
                description: Some("Runs quality analysis every 30min in background."),
                tag: None,
                gap,
            },
        );
        state.quality_daemon_checkbox = Some(quality_check as usize);
        y -= gap;

        let divider = create_label(LabelConfig {
            frame: CGRect::new(&CGPoint::new(pad, y), &CGSize::new(field_w, 1.0)),
            text: String::new(),
            background_color: Some(ui_colors::surface_border()),
            ..Default::default()
        });
        let _: () = msg_send![divider, setAlphaValue: 0.9f64];
        add_subview(container, divider);
        y -= gap;

        let user_hint = create_label(LabelConfig {
            frame: CGRect::new(&CGPoint::new(pad, y), &CGSize::new(field_w, 16.0)),
            text: "Assistant endpoints and models are configured in Keys tab.".to_string(),
            font_size: ui_tokens::MICRO_FONT_SIZE,
            text_color: secondary,
            ..Default::default()
        });
        add_subview(container, user_hint);

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
        let mut runtime_config = hotkeys::HotkeyRuntimeConfig::from(&config);
        runtime_config.hold_mods = mods;

        // If DoubleCtrl toggle is enabled, Ctrl-only hold is unsafe → disable toggle.
        if mods == HoldMods::Ctrl && config.toggle_trigger == ToggleTrigger::DoubleCtrl {
            let _ = config.save_to_env_many(&[
                ("HOLD_MODS", value),
                ("TOGGLE_TRIGGER", ToggleTrigger::None.as_str()),
            ]);
            runtime_config.toggle_trigger = ToggleTrigger::None;

            let state = BOOTSTRAP_STATE.lock().unwrap_or_else(|e| e.into_inner());
            set_keys_popup_index(state.keys_toggle_popup, 0);
        } else {
            let _ = config.save_to_env("HOLD_MODS", value);
        }
        hotkeys::apply_hotkey_runtime_config(runtime_config);
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
                let mut runtime_config = hotkeys::HotkeyRuntimeConfig::from(&config);
                runtime_config.hold_mods = HoldMods::Fn;
                runtime_config.toggle_trigger = ToggleTrigger::DoubleOption;
                runtime_config.hold_exclusive = false;
                hotkeys::apply_hotkey_runtime_config(runtime_config);

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
                let mut runtime_config = hotkeys::HotkeyRuntimeConfig::from(&config);
                runtime_config.hold_mods = HoldMods::Fn;
                runtime_config.toggle_trigger = ToggleTrigger::None;
                runtime_config.hold_exclusive = true;
                hotkeys::apply_hotkey_runtime_config(runtime_config);

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
        let mut runtime_config = hotkeys::HotkeyRuntimeConfig::from(&config);
        runtime_config.hold_exclusive = hold_exclusive;
        hotkeys::apply_hotkey_runtime_config(runtime_config);
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
        let mut runtime_config = hotkeys::HotkeyRuntimeConfig::from(&config);
        runtime_config.toggle_trigger = trigger;

        // If enabling DoubleCtrl and hold is Ctrl-only, switch to Ctrl+Option and enable modes.
        if trigger == ToggleTrigger::DoubleCtrl && config.hold_mods == HoldMods::Ctrl {
            let _ = config.save_to_env_many(&[
                ("HOLD_MODS", HoldMods::CtrlAlt.as_str()),
                ("HOLD_EXCLUSIVE", "0"),
            ]);
            runtime_config.hold_mods = HoldMods::CtrlAlt;
            runtime_config.hold_exclusive = false;

            let state = BOOTSTRAP_STATE.lock().unwrap_or_else(|e| e.into_inner());
            set_keys_popup_index(state.keys_hold_popup, 2);
            set_keys_checkbox_state(state.keys_exclusive_checkbox, true);
        }
        hotkeys::apply_hotkey_runtime_config(runtime_config);

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
        let mut runtime_config = hotkeys::HotkeyRuntimeConfig::from(&config);
        runtime_config.hold_start_delay_ms = ms;
        hotkeys::apply_hotkey_runtime_config(runtime_config);
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
        let mut runtime_config = hotkeys::HotkeyRuntimeConfig::from(&config);
        runtime_config.double_tap_interval_ms = ms;
        hotkeys::apply_hotkey_runtime_config(runtime_config);
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

pub(super) extern "C" fn on_show_dock_icon_toggled(
    _this: &Object,
    _cmd: objc::runtime::Sel,
    sender: Id,
) {
    unsafe {
        let state: isize = msg_send![sender, state];
        let enabled = state == 1;
        info!("Settings: show dock icon -> {}", enabled);
        let config = Config::load();
        let _ = config.save_to_env("SHOW_DOCK_ICON", if enabled { "1" } else { "0" });
        crate::apply_dock_icon_visibility(enabled);
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

pub(super) extern "C" fn on_ultra_quality_toggled(
    _this: &Object,
    _cmd: objc::runtime::Sel,
    sender: Id,
) {
    unsafe {
        let state: isize = msg_send![sender, state];
        let enabled = state == 1;
        info!("Settings: ultra quality final pass -> {}", enabled);
        let config = Config::load();
        let _ = config.save_to_env(
            "CODESCRIBE_LOCAL_STT_FINAL_PASS",
            if enabled { "1" } else { "0" },
        );
    }
}

pub(super) extern "C" fn on_permission_action(
    _this: &Object,
    _cmd: objc::runtime::Sel,
    sender: Id,
) {
    unsafe {
        let tag: isize = msg_send![sender, tag];
        if let Some(kind) = permission_kind_from_tag(tag) {
            info!("Settings: permission action for {:?}", kind);
            handle_permission_action(kind);
        }
    }
}

pub(super) extern "C" fn on_open_system_settings(
    _this: &Object,
    _cmd: objc::runtime::Sel,
    _sender: Id,
) {
    info!("Settings: opening System Settings");
    open_system_settings_security();
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
    fn toggle_row_spacing_is_consistent() {
        let gap = ui_tokens::DENSITY_MEDIUM;
        assert_eq!(toggle_row_step(false, gap), TOGGLE_ROW_HEIGHT + gap);
        assert_eq!(
            toggle_row_step(true, gap),
            TOGGLE_ROW_DESC_OFFSET + TOGGLE_ROW_DESC_HEIGHT + gap
        );
    }

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
    fn creator_setup_card_reflects_setup_progress() {
        let in_progress = creator_setup_card(false, 2, 5);
        assert_eq!(in_progress.title, "Setup in Progress");
        assert!(in_progress.subtitle.contains("2/5"));

        let ready = creator_setup_card(false, 5, 5);
        assert_eq!(ready.title, "Ready to Finish");
        assert!(ready.preview.contains("Finish Setup"));

        let drifted = creator_setup_card(true, 4, 5);
        assert_eq!(drifted.title, "Permissions Drifted");
    }

    #[test]
    fn creator_hotkey_card_summarizes_mode_bindings() {
        let settings = crate::config::UserSettings {
            mode_bindings: Some(vec![
                crate::config::ModeBinding {
                    mode: crate::config::WorkMode::Dictation,
                    binding: crate::config::ShortcutBinding::HoldCtrl,
                },
                crate::config::ModeBinding {
                    mode: crate::config::WorkMode::Formatting,
                    binding: crate::config::ShortcutBinding::DoubleLeftOption,
                },
                crate::config::ModeBinding {
                    mode: crate::config::WorkMode::Assistive,
                    binding: crate::config::ShortcutBinding::DoubleRightOption,
                },
            ]),
            ..Default::default()
        };

        let card = creator_hotkey_card(&settings);
        assert_eq!(card.title, "Mode Bindings");
        assert!(card.subtitle.contains("Hold Ctrl"));
        assert!(card.preview.contains("2x L-Option"));
        assert!(card.preview.contains("2x R-Option"));
    }

    #[test]
    fn creator_runtime_card_reports_quality_truth() {
        let config = crate::config::Config {
            ai_formatting_enabled: true,
            show_dock_icon: false,
            use_local_stt: true,
            ..Default::default()
        };
        let quality_state = crate::quality_loop::QualityDaemonState {
            pending_mismatches: 3,
            available: true,
            ..Default::default()
        };

        let card = creator_runtime_card(&config, &quality_state);
        assert_eq!(card.title, "Runtime Truth");
        assert!(card.subtitle.contains("Local Whisper"));
        assert!(card.preview.contains("Formatting: on"));
        assert!(card.preview.contains("Quality: 3 pending"));
        assert!(card.preview.contains("Dock: hidden"));
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
