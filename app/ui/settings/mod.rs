//! Native AppKit Settings window.
//!
//! # Safety
//!
//! Every `unsafe` block / function in this module shares one invariant matrix:
//!
//! 1. **Main-thread affinity.** AppKit objects (`NSWindow`, `NSView`, `NSButton`,
//!    etc.) MUST only be addressed from the main thread. All entry points here
//!    are reached either directly from the main runloop or via
//!    `Queue::main().exec_async(...)`, which trampolines onto the main thread
//!    before the closure body executes.
//! 2. **Object validity.** `Id = *mut Object` parameters and the values returned
//!    by `[cls new]` / `[cls alloc] init...]` are non-null retained pointers
//!    obtained on the main thread and not yet released. Subviews/controls owned
//!    by their parent window/view live as long as the parent.
//! 3. **Selector / message arity.** `msg_send!` invocations bind to documented
//!    AppKit / Foundation selectors with matching argument types. The
//!    `extern_class!`-style declarations in `objc2_app_kit` provide the source
//!    of truth for selector signatures.
//! 4. **Environment mutation.** `std::env::remove_var` / `set_var` calls in this
//!    module run synchronously on the main thread before any worker spawns; no
//!    parallel reader is in flight (Rust 2024 soundness contract).
//!
//! Per-block `// SAFETY:` annotations call out additional invariants where the
//! pattern deviates (e.g. raw FFI, retain-count balancing, cross-thread hops).

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
use objc2_app_kit::{NSVisualEffectMaterial, NSWindowButton, NSWindowToolbarStyle};
use tracing::{info, warn};

use crate::config::{Config, ShortcutBinding, UserSettings, WorkMode, keychain};
use crate::ipc::{IpcCommand, IpcResponse};
use crate::os::permissions::PermissionStatus;
use crate::os::{hotkeys, shortcut_registry};
use crate::ui::onboarding::{
    PERMISSION_ORDER, PermissionKind, open_permission_settings, permission_status,
    reconcile_permission_runtime_after_grant, request_permission,
};
use crate::ui::settings::handlers::{
    action_handler_class, toolbar_delegate_class, window_delegate_class,
};
use crate::ui_helpers::{
    LabelConfig, add_subview, apply_shared_shell_panel_policy, button_set_action, button_style,
    create_button, create_glass_effect_view_with, create_label, create_scrollable_text_view,
    create_secure_text_input, create_segmented_control, create_slider, create_text_input,
    create_toggle, get_text_view_string, layout_region_frame_for_view, ns_string,
    present_shared_shell_panel, set_glass_effect_content_view, set_text_field_string,
    set_text_view_string, settings_shell_panel_policy, ui_colors, ui_tokens, window_close,
    window_content_view,
};

mod handlers;

mod actions;
mod ai_prompts_tab;
mod audio_input_tab;
mod creator_tab;
mod dashboards;
mod engine_tab;
mod hotkey_conflicts;
mod keychain_status;
mod mode_bindings;
mod modes_tab;
mod permissions;
mod preview;
mod prompts;
mod quality_tab;
mod rows;

use actions::*;
use ai_prompts_tab::*;
use audio_input_tab::*;
use creator_tab::*;
use dashboards::*;
use engine_tab::*;
use hotkey_conflicts::*;
use keychain_status::*;
use mode_bindings::*;
use modes_tab::*;
use permissions::*;
use preview::*;
use prompts::*;
use quality_tab::*;
use rows::*;

// Type alias for Objective-C object pointers
use crate::ui_helpers::Id;

const SIDEBAR_WIDTH: f64 = 216.0;
const SETTINGS_WINDOW_WIDTH: f64 = 840.0;
const SETTINGS_WINDOW_HEIGHT: f64 = 700.0;
const SETTINGS_CONTENT_INSET_X: f64 = 20.0;
const SETTINGS_CONTENT_INSET_Y: f64 = 20.0;
const TAB_BUTTON_HEIGHT: f64 = 34.0;
const TAB_BUTTON_GAP: f64 = 6.0;
const SIDEBAR_GROUP_GAP: f64 = 12.0;
const TAB_ACTIVE_BG_ALPHA: f64 = 0.10;
const TAB_ACTIVE_BORDER_ALPHA: f64 = 0.22;
const SIDEBAR_INSET: f64 = 12.0;
const SETTINGS_TITLEBAR_SAFE_INSET: f64 = 56.0;
const SETTINGS_TAB_START_OFFSET: f64 = 20.0;
const TAB_CREATOR: usize = 0;
const TAB_KEYS: usize = 1;
const TAB_AUDIO: usize = 2;
const TAB_VOICE_LAB: usize = 3;
const TAB_ENGINE: usize = 4;
const TAB_USER: usize = 5;
const TAB_COUNT: usize = 6;

const TOGGLE_ROW_HEIGHT: f64 = 22.0;
const TOGGLE_SWITCH_WIDTH: f64 = 38.0;
const TOGGLE_SWITCH_HEIGHT: f64 = 22.0;
const TOGGLE_ROW_LABEL_INDENT: f64 = 0.0;
const TOGGLE_ROW_DESC_OFFSET: f64 = 18.0;
const TOGGLE_ROW_DESC_HEIGHT: f64 = 16.0;
const SETTINGS_INPUT_HEIGHT: f64 = 22.0;
const KEY_STATUS_ICON_SIZE: f64 = 14.0;
const PREVIEW_NO_OVERLAY_MIN_INTERIM_SEC: f32 = 8.0;
const PREVIEW_SAMPLE_UTTERANCE_SEC: f32 = 12.0;
const PREVIEW_SAMPLE_TEXT: &str =
    "Partiale mają być appendowane, poprawiamy tylko aktywny ogon, a nie kasujemy całego tekstu.";
const PROMPT_EDITOR_DESIRED_HEIGHT: f64 = 220.0;
const PROMPT_EDITOR_STATUS_HEIGHT: f64 = 16.0;
const PROMPT_EDITOR_BOTTOM_PADDING: f64 = 24.0;

const STEP_TEST_MIC: usize = 0;
const STEP_SHOW_OVERLAY: usize = 1;
const STEP_PRESS_HOTKEY: usize = 2;
const MODE_DICTATION_TAG: isize = 0;
const MODE_FORMATTING_TAG: isize = 1;
const MODE_ASSISTIVE_TAG: isize = 2;
const MODE_DISABLE_TAG_OFFSET: isize = 10;
const MODE_DICTATION_DOUBLE_CTRL_TAG: isize = 100;

#[inline]
fn objc_class(name: &'static str) -> &'static Class {
    Class::get(name).unwrap_or_else(|| panic!("Objective-C class not found: {name}"))
}

/// Compute a safe top inset from Tahoe/AppKit layout guides to keep sidebar controls
/// below titlebar chrome (traffic lights + unified toolbar), with a stable fallback.
fn settings_titlebar_safe_inset(view: Id, fallback: f64) -> f64 {
    if view.is_null() {
        return fallback;
    }

    // SAFETY: see module-level # Safety doc — main-thread AppKit / msg_send! access on retained `Id` pointers.
    unsafe {
        let bounds: CGRect = msg_send![view, bounds];
        if let Some(layout_frame) = layout_region_frame_for_view(view) {
            let top_inset =
                (bounds.size.height - (layout_frame.origin.y + layout_frame.size.height)).max(0.0);
            if top_inset.is_finite() {
                return (top_inset + ui_tokens::DENSITY_MEDIUM).max(fallback);
            }
        }
    }

    fallback
}

fn parse_env_bool(v: &str) -> bool {
    matches!(
        v.trim().to_ascii_lowercase().as_str(),
        "1" | "true" | "yes" | "on"
    )
}

#[derive(Default)]
struct SettingsWindowState {
    window: Option<usize>,
    window_delegate: Option<usize>,
    action_handler: Option<usize>,
    root_view: Option<usize>,
    step_labels: [Option<usize>; 3],
    tab_buttons: [Option<usize>; TAB_COUNT],
    content_views: [Option<usize>; TAB_COUNT],
    active_tab: usize,
    keys_mode_binding_labels: [Option<usize>; 3],
    keys_recorder_hint_label: Option<usize>,
    keys_conflict_label: Option<usize>,
    keys_conflict_details_button: Option<usize>,
    hold_delay_value_label: Option<usize>,
    double_tap_value_label: Option<usize>,
    preview_buffer_delay_value_label: Option<usize>,
    preview_buffer_delay_slider: Option<usize>,
    preview_typing_cps_value_label: Option<usize>,
    preview_typing_cps_slider: Option<usize>,
    preview_emit_words_max_value_label: Option<usize>,
    preview_emit_words_max_slider: Option<usize>,
    preview_interim_sec_value_label: Option<usize>,
    preview_interim_sec_slider: Option<usize>,
    preview_timing_summary_label: Option<usize>,
    preview_timing_text_view: Option<usize>,
    preview_preset_segment: Option<usize>,
    preview_env_override_label: Option<usize>,
    preview_advanced_button: Option<usize>,
    preview_advanced_rows: Vec<usize>,
    preview_advanced_expanded: bool,
    preview_timing_forced_custom: bool,
    config_cache: Option<Config>,
    // Onboarding additions
    permission_labels: [Option<usize>; 5],
    permission_action_buttons: [Option<usize>; 5],
    permission_requested: [bool; 5],
    permission_polling: bool,
    qube_daemon_checkbox: Option<usize>,
    ultra_quality_checkbox: Option<usize>,
    quality_available_label: Option<usize>,
    quality_pending_label: Option<usize>,
    quality_last_check_label: Option<usize>,
    qube_report_label: Option<usize>,
    quality_open_report_button: Option<usize>,
    llm_endpoint_field: Option<usize>,
    llm_model_field: Option<usize>,
    llm_key_field: Option<usize>,
    llm_key_status_icon: Option<usize>,
    llm_key_status_label: Option<usize>,
    assistive_endpoint_field: Option<usize>,
    assistive_model_field: Option<usize>,
    assistive_key_field: Option<usize>,
    assistive_key_status_icon: Option<usize>,
    assistive_key_status_label: Option<usize>,
    prompt_type_popup: Option<usize>,
    prompt_editor_text_view: Option<usize>,
    prompt_status_label: Option<usize>,
    prompt_path_label: Option<usize>,
    diagnostics_permission_labels: [Option<usize>; 5],
    diagnostics_conflict_label: Option<usize>,
    diagnostics_conflict_details_button: Option<usize>,
    diagnostics_status_label: Option<usize>,
}

lazy_static! {
    static ref SETTINGS_WINDOW_STATE: Mutex<SettingsWindowState> =
        Mutex::new(SettingsWindowState::default());
}

fn clear_settings_ui_state(state: &mut SettingsWindowState) {
    state.step_labels = [None, None, None];
    state.tab_buttons = [None; TAB_COUNT];
    state.content_views = [None; TAB_COUNT];
    state.active_tab = TAB_CREATOR;
    state.keys_mode_binding_labels = [None; 3];
    state.keys_recorder_hint_label = None;
    state.keys_conflict_label = None;
    state.keys_conflict_details_button = None;
    state.hold_delay_value_label = None;
    state.double_tap_value_label = None;
    state.preview_buffer_delay_value_label = None;
    state.preview_buffer_delay_slider = None;
    state.preview_typing_cps_value_label = None;
    state.preview_typing_cps_slider = None;
    state.preview_emit_words_max_value_label = None;
    state.preview_emit_words_max_slider = None;
    state.preview_interim_sec_value_label = None;
    state.preview_interim_sec_slider = None;
    state.preview_timing_summary_label = None;
    state.preview_timing_text_view = None;
    state.preview_preset_segment = None;
    state.preview_env_override_label = None;
    state.preview_advanced_button = None;
    state.preview_advanced_rows.clear();
    state.preview_advanced_expanded = false;
    state.preview_timing_forced_custom = false;
    state.permission_labels = [None, None, None, None, None];
    state.permission_action_buttons = [None, None, None, None, None];
    state.permission_requested = [false; 5];
    state.permission_polling = false;
    state.qube_daemon_checkbox = None;
    state.ultra_quality_checkbox = None;
    state.quality_available_label = None;
    state.quality_pending_label = None;
    state.quality_last_check_label = None;
    state.qube_report_label = None;
    state.quality_open_report_button = None;
    state.llm_endpoint_field = None;
    state.llm_model_field = None;
    state.llm_key_field = None;
    state.llm_key_status_icon = None;
    state.llm_key_status_label = None;
    state.assistive_endpoint_field = None;
    state.assistive_model_field = None;
    state.assistive_key_field = None;
    state.assistive_key_status_icon = None;
    state.assistive_key_status_label = None;
    state.prompt_type_popup = None;
    state.prompt_editor_text_view = None;
    state.prompt_status_label = None;
    state.prompt_path_label = None;
    state.diagnostics_permission_labels = [None; 5];
    state.diagnostics_conflict_label = None;
    state.diagnostics_conflict_details_button = None;
    state.diagnostics_status_label = None;
}

static SHOW_SETTINGS_WINDOW_IN_FLIGHT: AtomicBool = AtomicBool::new(false);

unsafe fn present_settings_window(window: Id) {
    // SAFETY: see module-level # Safety doc — main-thread AppKit / msg_send! access on retained `Id` pointers.
    unsafe { present_shared_shell_panel(window) };
}

/// Show the persistent Settings window.
pub fn show_settings_window() {
    // Fast path: if window already exists, just show it on main thread.
    {
        let state = SETTINGS_WINDOW_STATE
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        if let Some(ptr) = state.window {
            drop(state);
            Queue::main().exec_async(move || unsafe {
                let window = ptr as Id;
                present_settings_window(window);
                refresh_permission_indicators();
                start_permission_polling();
            });
            return;
        }
    }

    // Slow path: need to create window — guard against concurrent thread spawns.
    if SHOW_SETTINGS_WINDOW_IN_FLIGHT.swap(true, Ordering::SeqCst) {
        return;
    }
    std::thread::spawn(|| {
        let config = Config::load();
        Queue::main().exec_async(move || {
            SHOW_SETTINGS_WINDOW_IN_FLIGHT.store(false, Ordering::SeqCst);
            let mut state = SETTINGS_WINDOW_STATE
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            state.config_cache = Some(config);
            drop(state);
            show_settings_window_impl();
        });
    });
}

fn show_settings_window_impl() {
    // Keep Settings as a standalone window.
    // It should not depend on the voice chat overlay being available.
    // (This also avoids deadlocks when the overlay is mid-layout.)
    // SAFETY: see module-level # Safety doc — main-thread AppKit / msg_send! access on retained `Id` pointers.
    unsafe {
        let reuse_window = {
            let mut state = SETTINGS_WINDOW_STATE
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            if let Some(window_ptr) = state.window {
                let ns_window = objc_class("NSWindow");
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
            present_settings_window(window);
            refresh_permission_indicators();
            start_permission_polling();
            return;
        }

        let ns_screen = objc_class("NSScreen");
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

        let fixed_size = CGSize::new(window_width, window_height);
        let shell_policy = settings_shell_panel_policy(fixed_size);
        let ns_window = objc_class("NSWindow");
        let window: Id = msg_send![ns_window, alloc];
        let window: Id = msg_send![
            window,
            initWithContentRect: frame
            styleMask: shell_policy.style_mask
            backing: shell_policy.backing_store
            defer: false
        ];

        // Keep Settings on a conventional AppKit preferences shell. The content
        // panes use semantic NSVisualEffect materials; the window itself stays native.
        let _: () = msg_send![window, setTitle: ns_string("Settings")];
        apply_shared_shell_panel_policy(window, &shell_policy);
        let toolbar_delegate_class = toolbar_delegate_class();
        let toolbar_delegate: Id = msg_send![toolbar_delegate_class, new];
        let ns_toolbar = objc_class("NSToolbar");
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
            let _: () = msg_send![window, setToolbarStyle: NSWindowToolbarStyle::Preference];
        }
        let supports_toolbar_button: bool =
            msg_send![window, respondsToSelector: sel!(setShowsToolbarButton:)];
        if supports_toolbar_button {
            let _: () = msg_send![window, setShowsToolbarButton: false];
        }
        // Hard lock the size (no resize handles, no zoom).
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
            let mut state = SETTINGS_WINDOW_STATE
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            state.window = Some(window as usize);
            state.window_delegate = Some(window_delegate as usize);
        } // Release lock before AppKit call to avoid nested-runloop deadlock.

        present_settings_window(window);
        refresh_permission_indicators();
        start_permission_polling();
    }
}

/// Attach the Settings view inside an existing parent view.
///
/// # Safety
/// `parent` must be a valid `NSView` instance owned by AppKit.
unsafe fn attach_settings_view(parent: Id, frame: core_graphics::geometry::CGRect) -> Option<Id> {
    // SAFETY: see module-level # Safety doc — main-thread AppKit / msg_send! access on retained `Id` pointers.
    unsafe {
        let (config, existing_root) = {
            let state = SETTINGS_WINDOW_STATE
                .lock()
                .unwrap_or_else(|e| e.into_inner());
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
        let ns_view = objc_class("NSView");
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

        let mut state = SETTINGS_WINDOW_STATE
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        state.root_view = Some(root as usize);
        state.action_handler = Some(action_handler as usize);
        state.step_labels = built_state.step_labels;
        state.tab_buttons = built_state.tab_buttons;
        state.content_views = built_state.content_views;
        state.active_tab = built_state.active_tab;
        state.keys_mode_binding_labels = built_state.keys_mode_binding_labels;
        state.keys_recorder_hint_label = built_state.keys_recorder_hint_label;
        state.keys_conflict_label = built_state.keys_conflict_label;
        state.keys_conflict_details_button = built_state.keys_conflict_details_button;
        state.hold_delay_value_label = built_state.hold_delay_value_label;
        state.double_tap_value_label = built_state.double_tap_value_label;
        state.preview_buffer_delay_value_label = built_state.preview_buffer_delay_value_label;
        state.preview_buffer_delay_slider = built_state.preview_buffer_delay_slider;
        state.preview_typing_cps_value_label = built_state.preview_typing_cps_value_label;
        state.preview_typing_cps_slider = built_state.preview_typing_cps_slider;
        state.preview_emit_words_max_value_label = built_state.preview_emit_words_max_value_label;
        state.preview_emit_words_max_slider = built_state.preview_emit_words_max_slider;
        state.preview_interim_sec_value_label = built_state.preview_interim_sec_value_label;
        state.preview_interim_sec_slider = built_state.preview_interim_sec_slider;
        state.preview_timing_summary_label = built_state.preview_timing_summary_label;
        state.preview_timing_text_view = built_state.preview_timing_text_view;
        state.preview_preset_segment = built_state.preview_preset_segment;
        state.preview_env_override_label = built_state.preview_env_override_label;
        state.preview_advanced_button = built_state.preview_advanced_button;
        state.preview_advanced_rows = built_state.preview_advanced_rows;
        state.preview_advanced_expanded = built_state.preview_advanced_expanded;
        state.config_cache = built_state.config_cache;
        state.permission_labels = built_state.permission_labels;
        state.permission_action_buttons = built_state.permission_action_buttons;
        state.permission_requested = built_state.permission_requested;
        state.permission_polling = built_state.permission_polling;
        state.qube_daemon_checkbox = built_state.qube_daemon_checkbox;
        state.ultra_quality_checkbox = built_state.ultra_quality_checkbox;
        state.quality_available_label = built_state.quality_available_label;
        state.quality_pending_label = built_state.quality_pending_label;
        state.quality_last_check_label = built_state.quality_last_check_label;
        state.qube_report_label = built_state.qube_report_label;
        state.quality_open_report_button = built_state.quality_open_report_button;
        state.llm_endpoint_field = built_state.llm_endpoint_field;
        state.llm_model_field = built_state.llm_model_field;
        state.llm_key_field = built_state.llm_key_field;
        state.llm_key_status_icon = built_state.llm_key_status_icon;
        state.llm_key_status_label = built_state.llm_key_status_label;
        state.assistive_endpoint_field = built_state.assistive_endpoint_field;
        state.assistive_model_field = built_state.assistive_model_field;
        state.assistive_key_field = built_state.assistive_key_field;
        state.assistive_key_status_icon = built_state.assistive_key_status_icon;
        state.assistive_key_status_label = built_state.assistive_key_status_label;
        state.prompt_type_popup = built_state.prompt_type_popup;
        state.prompt_editor_text_view = built_state.prompt_editor_text_view;
        state.prompt_status_label = built_state.prompt_status_label;
        state.prompt_path_label = built_state.prompt_path_label;
        state.diagnostics_permission_labels = built_state.diagnostics_permission_labels;
        state.diagnostics_conflict_label = built_state.diagnostics_conflict_label;
        state.diagnostics_conflict_details_button = built_state.diagnostics_conflict_details_button;
        state.diagnostics_status_label = built_state.diagnostics_status_label;

        drop(state); // Release lock before permission calls to avoid deadlock.

        refresh_hotkey_conflict_indicator();
        refresh_quality_dashboard();
        refresh_diagnostics_dashboard();
        refresh_prompt_editor_labels();
        refresh_transcription_preview_panel();
        refresh_permission_indicators();
        start_permission_polling();
        Some(root)
    }
}

// ============================================================================
// Permission checks / onboarding readiness
// ============================================================================

unsafe fn build_settings_ui(
    root_view: Id,
    settings_width: f64,
    settings_height: f64,
    action_handler: Id,
    config: &Config,
) -> SettingsWindowState {
    // SAFETY: see module-level # Safety doc — main-thread AppKit / msg_send! access on retained `Id` pointers.
    unsafe {
        use core_graphics::geometry::{CGPoint, CGRect, CGSize};
        let ns_view = objc_class("NSView");
        let mut state = SettingsWindowState::default();

        let settings_width = settings_width.max(SIDEBAR_WIDTH + 240.0);
        let settings_height = settings_height.max(280.0);
        let body_h = settings_height;

        let root_content: Id = msg_send![ns_view, alloc];
        let root_content: Id = msg_send![
            root_content,
            initWithFrame: CGRect::new(
                &CGPoint::new(0.0, 0.0),
                &CGSize::new(settings_width, settings_height),
            )
        ];
        let _: () = msg_send![
            root_content,
            setAutoresizingMask: 2_isize | 16_isize // Width | Height
        ];
        add_subview(root_view, root_content);

        // Left: Sidebar glass pane
        let sidebar_frame =
            CGRect::new(&CGPoint::new(0.0, 0.0), &CGSize::new(SIDEBAR_WIDTH, body_h));
        let sidebar_glass = create_glass_effect_view_with(
            sidebar_frame,
            NSVisualEffectMaterial::Sidebar,
            objc2_app_kit::NSVisualEffectBlendingMode::WithinWindow,
            objc2_app_kit::NSVisualEffectState::FollowsWindowActiveState,
        );
        let _: () = msg_send![
            sidebar_glass,
            setAutoresizingMask: 16_isize | 4_isize // Height | MaxXMargin
        ];
        let sidebar_glass_layer: Id = msg_send![sidebar_glass, layer];
        if !sidebar_glass_layer.is_null() {
            let _: () = msg_send![sidebar_glass_layer, setMasksToBounds: true];
        }
        add_subview(root_content, sidebar_glass);

        let sidebar_container: Id = msg_send![ns_view, alloc];
        let sidebar_container: Id = msg_send![
            sidebar_container,
            initWithFrame: CGRect::new(
                &CGPoint::new(0.0, 0.0),
                &CGSize::new(SIDEBAR_WIDTH, body_h),
            )
        ];
        let _: () = msg_send![
            sidebar_container,
            setAutoresizingMask: 2_isize | 16_isize // Width | Height
        ];
        let _: bool = set_glass_effect_content_view(sidebar_glass, sidebar_container);

        // Right: Content glass pane
        let content_bg_frame = CGRect::new(
            &CGPoint::new(SIDEBAR_WIDTH, 0.0),
            &CGSize::new(settings_width - SIDEBAR_WIDTH, body_h),
        );
        let content_glass = create_glass_effect_view_with(
            content_bg_frame,
            NSVisualEffectMaterial::ContentBackground,
            objc2_app_kit::NSVisualEffectBlendingMode::WithinWindow,
            objc2_app_kit::NSVisualEffectState::FollowsWindowActiveState,
        );
        let _: () = msg_send![
            content_glass,
            setAutoresizingMask: 2_isize | 16_isize // Width | Height
        ];
        let content_glass_layer: Id = msg_send![content_glass, layer];
        if !content_glass_layer.is_null() {
            let _: () = msg_send![content_glass_layer, setMasksToBounds: true];
        }
        add_subview(root_content, content_glass);

        let content_container: Id = msg_send![ns_view, alloc];
        let content_container: Id = msg_send![
            content_container,
            initWithFrame: CGRect::new(
                &CGPoint::new(0.0, 0.0),
                &CGSize::new(content_bg_frame.size.width, body_h),
            )
        ];
        let _: () = msg_send![
            content_container,
            setAutoresizingMask: 2_isize | 16_isize // Width | Height
        ];
        let _: bool = set_glass_effect_content_view(content_glass, content_container);

        let split_divider = create_label(LabelConfig {
            frame: CGRect::new(
                &CGPoint::new(SIDEBAR_WIDTH - 0.5, 0.0),
                &CGSize::new(1.0, body_h),
            ),
            text: String::new(),
            background_color: Some(ui_colors::header_border()),
            ..Default::default()
        });
        let _: () = msg_send![
            split_divider,
            setAutoresizingMask: 16_isize | 4_isize // Height | MaxXMargin
        ];
        add_subview(root_content, split_divider);

        let content_area_w = content_bg_frame.size.width;
        let content_area_h = body_h;

        // Offset sidebar content below the titlebar+toolbar zone (~52px in unified style).
        // With FullSizeContentView, content extends under the titlebar, so we must
        // manually avoid the traffic lights area.
        let titlebar_inset =
            settings_titlebar_safe_inset(root_content, SETTINGS_TITLEBAR_SAFE_INSET);

        // Sidebar tab buttons (inside sidebar container, no redundant title label)
        let tab_start_y = body_h - titlebar_inset - SETTINGS_TAB_START_OFFSET;
        let tab_names = ["Creator", "Keys", "Audio", "Voice Lab", "Engine", "User"];
        let tab_sels = [
            sel!(onTabCreator:),
            sel!(onTabKeys:),
            sel!(onTabAudio:),
            sel!(onTabVoiceLab:),
            sel!(onTabEngine:),
            sel!(onTabUser:),
        ];
        let mut tab_buttons: [Option<usize>; TAB_COUNT] = [None; TAB_COUNT];

        let mut cursor_y = tab_start_y;
        for (i, (name, sel)) in tab_names.iter().zip(tab_sels.iter()).enumerate() {
            if i == 1 || i == 4 {
                cursor_y -= SIDEBAR_GROUP_GAP / 2.0;
                let sep_line = create_label(LabelConfig {
                    frame: CGRect::new(
                        &CGPoint::new(SIDEBAR_INSET + 4.0, cursor_y),
                        &CGSize::new(SIDEBAR_WIDTH - SIDEBAR_INSET * 2.0 - 8.0, 1.0),
                    ),
                    text: String::new(),
                    background_color: Some(ui_colors::header_border()),
                    ..Default::default()
                });
                add_subview(sidebar_container, sep_line);
                cursor_y -= SIDEBAR_GROUP_GAP / 2.0;
            }

            cursor_y -= TAB_BUTTON_HEIGHT;
            let btn_frame = CGRect::new(
                &CGPoint::new(SIDEBAR_INSET, cursor_y),
                &CGSize::new(SIDEBAR_WIDTH - SIDEBAR_INSET * 2.0, TAB_BUTTON_HEIGHT),
            );

            let tab_btn = create_sidebar_tab_button(btn_frame, name, i == TAB_CREATOR);
            button_set_action(tab_btn, action_handler, *sel);
            add_subview(sidebar_container, tab_btn);
            tab_buttons[i] = Some(tab_btn as usize);
            cursor_y -= TAB_BUTTON_GAP;
        }

        // ====================================================================
        // Content area views (one per tab, inside content container)
        // ====================================================================
        // Relative to content container: origin is (0,0)
        let tab_content_frame = CGRect::new(
            &CGPoint::new(SETTINGS_CONTENT_INSET_X, SETTINGS_CONTENT_INSET_Y),
            &CGSize::new(
                (content_area_w - SETTINGS_CONTENT_INSET_X * 2.0).max(240.0),
                (content_area_h - SETTINGS_CONTENT_INSET_Y - (titlebar_inset + 8.0)).max(220.0),
            ),
        );

        let tab_document_frame = CGRect::new(
            &CGPoint::new(0.0, 0.0),
            &CGSize::new(tab_content_frame.size.width, tab_content_frame.size.height),
        );

        // --- Creator tab (index 0) ---
        let creator_view =
            build_creator_tab(action_handler, tab_document_frame, config, &mut state);
        let creator_scroll = wrap_tab_content_in_scroll_view(tab_content_frame, creator_view);
        add_subview(content_container, creator_scroll);

        // --- Keys tab (index 1) ---
        let keys_view =
            build_modes_shortcuts_tab(action_handler, tab_document_frame, config, &mut state);
        let keys_scroll = wrap_tab_content_in_scroll_view(tab_content_frame, keys_view);
        let _: () = msg_send![keys_scroll, setHidden: true];
        add_subview(content_container, keys_scroll);

        // --- Audio tab (index 2) ---
        let audio_view = build_audio_input_tab(action_handler, tab_document_frame, config);
        let audio_scroll = wrap_tab_content_in_scroll_view(tab_content_frame, audio_view);
        let _: () = msg_send![audio_scroll, setHidden: true];
        add_subview(content_container, audio_scroll);

        // --- Voice Lab tab (index 3) ---
        let voice_lab_view =
            build_quality_tab(action_handler, tab_document_frame, config, &mut state);
        let voice_lab_scroll = wrap_tab_content_in_scroll_view(tab_content_frame, voice_lab_view);
        let _: () = msg_send![voice_lab_scroll, setHidden: true];
        add_subview(content_container, voice_lab_scroll);

        // --- Engine tab (index 4) ---
        let engine_view = build_engine_tab(action_handler, tab_document_frame, config, &mut state);
        let engine_scroll = wrap_tab_content_in_scroll_view(tab_content_frame, engine_view);
        let _: () = msg_send![engine_scroll, setHidden: true];
        add_subview(content_container, engine_scroll);

        // --- User tab (index 5) ---
        let user_view =
            build_ai_prompts_tab(action_handler, tab_document_frame, config, &mut state);
        let user_scroll = wrap_tab_content_in_scroll_view(tab_content_frame, user_view);
        let _: () = msg_send![user_scroll, setHidden: true];
        add_subview(content_container, user_scroll);

        // ====================================================================
        // Store state
        // ====================================================================
        state.tab_buttons = tab_buttons;
        state.content_views = [
            Some(creator_scroll as usize),
            Some(keys_scroll as usize),
            Some(audio_scroll as usize),
            Some(voice_lab_scroll as usize),
            Some(engine_scroll as usize),
            Some(user_scroll as usize),
        ];
        state.active_tab = TAB_CREATOR;
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
    // SAFETY: see module-level # Safety doc — main-thread AppKit / msg_send! access on retained `Id` pointers.
    unsafe {
        let ns_button = objc_class("NSButton");
        let ns_font = objc_class("NSFont");

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
            "Creator" => "wand.and.stars",
            "Keys" => "keyboard",
            "Audio" => "speaker.wave.2",
            "Voice Lab" => "waveform",
            "Engine" => "stethoscope",
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
            let mut state = SETTINGS_WINDOW_STATE
                .lock()
                .unwrap_or_else(|e| e.into_inner());
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

        if index == TAB_CREATOR {
            refresh_permission_indicators();
        } else if index == TAB_VOICE_LAB {
            refresh_transcription_preview_panel();
        } else if index == TAB_ENGINE {
            refresh_quality_dashboard();
            refresh_diagnostics_dashboard();
        } else if index == TAB_USER {
            refresh_prompt_editor_labels();
        }
    });
}

pub(super) fn handle_test_mic() {
    update_step_status(STEP_TEST_MIC, "recording\u{2026}");

    if let Err(e) = send_ipc(IpcCommand::StartRecording { assistive: false }) {
        warn!("Settings test mic failed to start: {}", e);
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
    crate::ui::voice_chat::show_voice_chat_overlay();
    crate::ui::voice_chat::show_agent_tab();
    crate::ui::voice_chat::update_voice_chat_status("Listening...");
    update_step_status(STEP_SHOW_OVERLAY, "done");
}

pub(super) fn handle_hotkey_done() {
    update_step_status(STEP_PRESS_HOTKEY, "done");
}

pub(super) fn handle_settings_window_closed() {
    let (delegate_ptr, handler_ptr, window_ptr) = {
        let mut state = SETTINGS_WINDOW_STATE
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let delegate_ptr = state.window_delegate.take();
        let handler_ptr = state.action_handler.take();
        let window_ptr = state.window.take();
        state.root_view = None;
        clear_settings_ui_state(&mut state);
        state.config_cache = None;
        (delegate_ptr, handler_ptr, window_ptr)
    };

    // SAFETY: see module-level # Safety doc — main-thread AppKit / msg_send! access on retained `Id` pointers.
    unsafe {
        if let Some(ptr) = delegate_ptr {
            let _: () = msg_send![ptr as Id, release];
        }
        if let Some(ptr) = handler_ptr {
            let _: () = msg_send![ptr as Id, release];
        }
        if let Some(ptr) = window_ptr {
            let _: () = msg_send![ptr as Id, release];
        }
    }
}

pub fn hide_settings_surface() {
    Queue::main().exec_async(|| unsafe {
        let (window_ptr, root_ptr) = {
            let mut state = SETTINGS_WINDOW_STATE
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            state.permission_polling = false;
            // Do NOT take ownership of window/delegate/action_handler here.
            // The `windowWillClose:` notification fires `handle_settings_window_closed`,
            // which drains and releases all three. Releasing twice would crash.
            // For the embedded (root-only, no window) path, the parent owns lifecycle.
            (state.window, state.root_view)
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
    hide_settings_surface();
}

/// Reset embedded Settings view state when the overlay is destroyed.
pub fn reset_embedded_settings_state() {
    let (delegate_ptr, handler_ptr) = {
        let mut state = SETTINGS_WINDOW_STATE
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        if state.window.is_some() {
            return;
        }
        let delegate_ptr = state.window_delegate.take();
        let handler_ptr = state.action_handler.take();
        state.root_view = None;
        state.config_cache = None;
        clear_settings_ui_state(&mut state);
        (delegate_ptr, handler_ptr)
    };

    // SAFETY: see module-level # Safety doc — main-thread AppKit / msg_send! access on retained `Id` pointers.
    unsafe {
        if let Some(ptr) = delegate_ptr {
            let _: () = msg_send![ptr as Id, release];
        }
        if let Some(ptr) = handler_ptr {
            let _: () = msg_send![ptr as Id, release];
        }
    }
}

fn update_step_status(index: usize, text: &str) {
    let text = text.to_string();
    Queue::main().exec_async(move || unsafe {
        let state = SETTINGS_WINDOW_STATE
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        if let Some(label) = state.step_labels.get(index).and_then(|v| *v) {
            set_text_field_string(label as Id, &text);
        }
    });
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
    fn mode_binding_tag_helpers_decode_expected_actions() {
        assert_eq!(mode_from_tag(MODE_DICTATION_TAG), Some(WorkMode::Dictation));
        assert_eq!(
            mode_from_disable_tag(MODE_FORMATTING_TAG + MODE_DISABLE_TAG_OFFSET),
            Some(WorkMode::Formatting)
        );
        assert!(mode_from_double_ctrl_tag(MODE_DICTATION_DOUBLE_CTRL_TAG));
        assert!(!mode_from_double_ctrl_tag(MODE_ASSISTIVE_TAG));
    }

    #[test]
    fn mode_binding_selection_rejects_invalid_mode_binding_pairs() {
        let settings = UserSettings::default();
        let err =
            mode_binding_selection_error(WorkMode::Formatting, ShortcutBinding::HoldFn, &settings);
        assert!(err.is_some());
    }

    #[test]
    fn assistive_mode_accepts_hold_bindings() {
        for binding in [
            ShortcutBinding::HoldFn,
            ShortcutBinding::HoldCtrl,
            ShortcutBinding::HoldCtrlAlt,
            ShortcutBinding::HoldCtrlShift,
            ShortcutBinding::HoldCtrlCmd,
        ] {
            assert!(
                mode_accepts_binding(WorkMode::Assistive, binding),
                "assistive should accept {:?}",
                binding
            );
        }
    }

    #[test]
    fn binding_from_recorded_event_maps_assistive_hold_ctrl_cmd() {
        // NSEventModifierFlagControl | NSEventModifierFlagCommand
        let flags = (1_u64 << 18) | (1_u64 << 20);
        let binding = binding_from_recorded_event(
            WorkMode::Assistive,
            12, // NSEventTypeFlagsChanged
            59, // Left Control
            flags,
        );
        assert_eq!(binding, Some(ShortcutBinding::HoldCtrlCmd));
    }

    #[test]
    fn mode_binding_selection_blocks_option_modes_when_dictation_is_double_ctrl() {
        let settings = UserSettings {
            mode_bindings: Some(vec![
                crate::config::ModeBinding {
                    mode: WorkMode::Dictation,
                    binding: ShortcutBinding::DoubleCtrl,
                },
                crate::config::ModeBinding {
                    mode: WorkMode::Formatting,
                    binding: ShortcutBinding::Disabled,
                },
                crate::config::ModeBinding {
                    mode: WorkMode::Assistive,
                    binding: ShortcutBinding::Disabled,
                },
            ]),
            ..Default::default()
        };
        assert_eq!(
            settings.mode_binding_for(WorkMode::Dictation),
            ShortcutBinding::DoubleCtrl
        );
        let err = mode_binding_selection_error(
            WorkMode::Assistive,
            ShortcutBinding::DoubleRightOption,
            &settings,
        );
        assert!(err.is_some());
    }

    #[test]
    fn hotkey_conflict_details_text_renders_all_conflicts() {
        let details = hotkey_conflict_details_text(&[
            shortcut_registry::HotkeyConflict {
                gesture: shortcut_registry::HotkeyGesture::HoldFn,
                message: "Conflicts with Show Emoji & Symbols (macOS #160).".to_string(),
            },
            shortcut_registry::HotkeyConflict {
                gesture: shortcut_registry::HotkeyGesture::ToggleDoubleCtrl,
                message: "Collides with Hold Ctrl.".to_string(),
            },
        ]);
        assert!(details.contains("1. Hold Fn/Globe -> Conflicts with Show Emoji & Symbols"));
        assert!(details.contains("2. Double-tap Ctrl -> Collides with Hold Ctrl."));
    }

    #[test]
    fn hotkey_conflict_details_text_handles_empty_list() {
        assert_eq!(
            hotkey_conflict_details_text(&[]),
            "No conflicts detected in current mode shortcuts."
        );
    }

    #[test]
    fn preview_effective_interim_sec_clamps_without_overlay() {
        assert_eq!(preview_effective_interim_sec(true, 1.2), 1.2);
        assert_eq!(
            preview_effective_interim_sec(false, 1.2),
            PREVIEW_NO_OVERLAY_MIN_INTERIM_SEC
        );
    }

    fn model_for_preset(values: PreviewTimingValues) -> PreviewTimingModel {
        PreviewTimingModel {
            overlay_enabled: true,
            buffer_delay_ms: values.buffer_delay_ms,
            typing_cps: values.typing_cps,
            emit_words_max: values.emit_words_max,
            requested_interim_sec: values.interim_sec,
            effective_interim_sec: values.interim_sec,
        }
    }

    #[test]
    fn preview_preset_values_anchor_smooth_operator_default() {
        let smooth = preset_values(PreviewTimingPreset::Smooth).expect("smooth preset has values");
        assert_eq!(smooth.buffer_delay_ms, 1038);
        assert!((smooth.typing_cps - 10.6).abs() < f32::EPSILON);
        assert_eq!(smooth.emit_words_max, 5);
        assert!((smooth.interim_sec - 8.0).abs() < f32::EPSILON);
    }

    #[test]
    fn preview_detect_preset_recognizes_presets_and_custom() {
        for preset in [
            PreviewTimingPreset::Smooth,
            PreviewTimingPreset::Snappy,
            PreviewTimingPreset::Relaxed,
        ] {
            let values = preset_values(preset).expect("timing preset has values");
            assert_eq!(detect_preset(model_for_preset(values)), preset);
        }

        let smooth = preset_values(PreviewTimingPreset::Smooth).expect("smooth preset has values");
        let off_model = PreviewTimingModel {
            overlay_enabled: false,
            ..model_for_preset(smooth)
        };
        assert_eq!(detect_preset(off_model), PreviewTimingPreset::Off);

        let custom_model = PreviewTimingModel {
            buffer_delay_ms: smooth.buffer_delay_ms + 25,
            ..model_for_preset(smooth)
        };
        assert_eq!(detect_preset(custom_model), PreviewTimingPreset::Custom);
    }

    #[test]
    fn preview_detect_preset_allows_small_tolerance() {
        let smooth = preset_values(PreviewTimingPreset::Smooth).expect("smooth preset has values");
        let near_smooth = PreviewTimingModel {
            buffer_delay_ms: smooth.buffer_delay_ms + 5,
            typing_cps: smooth.typing_cps + 0.1,
            requested_interim_sec: smooth.interim_sec - 0.1,
            effective_interim_sec: smooth.interim_sec - 0.1,
            ..model_for_preset(smooth)
        };
        assert_eq!(detect_preset(near_smooth), PreviewTimingPreset::Smooth);
    }

    #[test]
    fn preview_emit_chunks_respect_emit_words_cap() {
        let chunks = preview_emit_chunks("Partiale mają być appendowane teraz", 2);
        assert_eq!(
            chunks,
            vec![
                "Partiale mają ".to_string(),
                "być appendowane ".to_string(),
                "teraz".to_string()
            ]
        );
    }

    #[test]
    fn preview_timing_steps_grow_visible_text_monotonically() {
        let model = PreviewTimingModel {
            overlay_enabled: true,
            buffer_delay_ms: 280,
            typing_cps: 90.0,
            emit_words_max: 2,
            requested_interim_sec: 1.2,
            effective_interim_sec: 1.2,
        };
        let steps = preview_timing_steps(model);
        assert!(!steps.is_empty(), "preview should produce visible steps");
        for pair in steps.windows(2) {
            assert!(
                pair[1].visible_text.starts_with(&pair[0].visible_text),
                "preview should append forward, not reset"
            );
            assert!(
                pair[1].visible_at_ms >= pair[0].visible_at_ms,
                "preview timestamps should be monotonic"
            );
        }
        let report = preview_timing_report_text(model);
        assert!(report.contains("Chunker partial targets"));
        assert!(report.contains("Overlay-visible text"));
    }

    #[test]
    fn prompt_editor_layout_does_not_overlap_fields_above() {
        let gap = 10.0;
        let controls_bottom_y = 220.0;
        let layout = compute_prompt_editor_layout(controls_bottom_y, gap);
        let editor_top = layout.editor_y + layout.editor_height;

        assert!(
            editor_top <= controls_bottom_y + f64::EPSILON,
            "editor overlaps controls above"
        );
        assert!(
            layout.status_y + gap <= layout.editor_y + f64::EPSILON,
            "status must remain below editor"
        );
    }

    #[test]
    #[serial]
    #[cfg(target_os = "macos")]
    fn attach_settings_view_builds_root_view() {
        if std::env::var("CODESCRIBE_UI_TESTS").is_err() {
            return;
        }
        // SAFETY: see module-level # Safety doc — main-thread AppKit / msg_send! access on retained `Id` pointers.
        unsafe {
            let ns_view = objc_class("NSView");
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

            reset_embedded_settings_state();
            let state = SETTINGS_WINDOW_STATE
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            assert!(state.root_view.is_none());
        }
    }
}
