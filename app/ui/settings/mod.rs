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
    create_secure_text_input, create_slider, create_text_input, create_toggle,
    get_text_view_string, layout_region_frame_for_view, ns_string, present_shared_shell_panel,
    set_glass_effect_content_view, set_text_field_string, set_text_view_string,
    settings_shell_panel_policy, ui_colors, ui_tokens, window_close, window_content_view,
};

mod handlers;

// Type alias for Objective-C object pointers
type Id = *mut Object;

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
const TAB_TRANSCRIPTION: usize = 0;
const TAB_MODES_SHORTCUTS: usize = 1;
const TAB_AI_PROMPTS: usize = 2;
const TAB_AUDIO_INPUT: usize = 3;
const TAB_DIAGNOSTICS: usize = 4;
const TAB_COUNT: usize = 5;

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

#[derive(Default)]
struct ModeBindingRecorderState {
    monitor_installed: bool,
    target_mode: Option<WorkMode>,
}

lazy_static! {
    static ref MODE_BINDING_RECORDER_STATE: Mutex<ModeBindingRecorderState> =
        Mutex::new(ModeBindingRecorderState::default());
}

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

#[derive(Clone, Copy)]
struct ToggleRowSpec<'a> {
    title: &'a str,
    checked: bool,
    action: objc::runtime::Sel,
    description: Option<&'a str>,
    tag: Option<isize>,
    gap: f64,
}

#[derive(Clone, Copy)]
struct SliderSettingRowSpec<'a> {
    title: &'a str,
    value_text: &'a str,
    min: f64,
    max: f64,
    current: f64,
    action: objc::runtime::Sel,
    gap: f64,
}

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

#[derive(Clone, Copy, Debug, PartialEq)]
struct PreviewTimingModel {
    overlay_enabled: bool,
    buffer_delay_ms: u64,
    typing_cps: f32,
    emit_words_max: usize,
    requested_interim_sec: f32,
    effective_interim_sec: f32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct PreviewTimingStep {
    publish_ms: u64,
    visible_at_ms: u64,
    target_text: String,
    visible_text: String,
}

fn preview_effective_interim_sec(overlay_enabled: bool, requested_interim_sec: f32) -> f32 {
    let requested = requested_interim_sec.clamp(1.0, 30.0);
    if overlay_enabled {
        requested
    } else {
        requested.max(PREVIEW_NO_OVERLAY_MIN_INTERIM_SEC)
    }
}

fn current_preview_timing_model() -> PreviewTimingModel {
    let config = Config::load();
    let settings = UserSettings::load();
    let requested_interim_sec = settings.buffered_interim_sec.unwrap_or(1.2);

    PreviewTimingModel {
        overlay_enabled: config.transcription_overlay_enabled,
        buffer_delay_ms: settings.buffer_delay_ms.unwrap_or(280),
        typing_cps: settings.typing_cps.unwrap_or(90.0).max(5.0),
        emit_words_max: settings.emit_words_max.unwrap_or(2).clamp(1, 10) as usize,
        requested_interim_sec,
        effective_interim_sec: preview_effective_interim_sec(
            config.transcription_overlay_enabled,
            requested_interim_sec,
        ),
    }
}

fn preview_tokenize_for_emit(text: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let chars: Vec<char> = text.chars().collect();
    let mut cursor = 0usize;

    while cursor < chars.len() {
        let mut token = String::new();
        if chars[cursor].is_whitespace() {
            while cursor < chars.len() && chars[cursor].is_whitespace() {
                token.push(chars[cursor]);
                cursor += 1;
            }
        } else {
            while cursor < chars.len() && !chars[cursor].is_whitespace() {
                token.push(chars[cursor]);
                cursor += 1;
            }
            while cursor < chars.len() && chars[cursor].is_whitespace() {
                token.push(chars[cursor]);
                cursor += 1;
            }
        }
        tokens.push(token);
    }
    if tokens.is_empty() && !text.is_empty() {
        tokens.push(text.to_string());
    }
    tokens
}

fn preview_emit_chunks(text: &str, emit_words_max: usize) -> Vec<String> {
    let tokens = preview_tokenize_for_emit(text);
    let mut chunks = Vec::new();
    let mut current_index = 0usize;

    while current_index < tokens.len() {
        let mut chunk = String::new();
        let mut words = 0usize;
        while current_index < tokens.len() {
            let token = &tokens[current_index];
            chunk.push_str(token);
            if token.chars().any(|c| !c.is_whitespace()) {
                words += 1;
            }
            current_index += 1;

            if words >= emit_words_max {
                if current_index < tokens.len() {
                    let next = &tokens[current_index];
                    if next.chars().all(|c| c.is_whitespace()) {
                        chunk.push_str(next);
                        current_index += 1;
                    }
                }
                break;
            }
        }

        if !chunk.is_empty() {
            chunks.push(chunk);
        }
    }

    chunks
}

fn preview_partial_targets(sample: &str, interim_sec: f32) -> Vec<(u64, String)> {
    let words: Vec<&str> = sample.split_whitespace().collect();
    if words.is_empty() {
        return Vec::new();
    }

    let total_duration = PREVIEW_SAMPLE_UTTERANCE_SEC.max(interim_sec);
    let total_words = words.len();
    let mut targets = Vec::new();
    let mut reveal_cursor = 0usize;
    let mut t = interim_sec;

    while t < total_duration {
        let progress = (t / total_duration).clamp(0.0, 1.0);
        let reveal_words = ((total_words as f32 * progress).ceil() as usize)
            .clamp(1, total_words)
            .max((reveal_cursor + 1).min(total_words));
        if reveal_words > reveal_cursor {
            reveal_cursor = reveal_words;
            targets.push((
                (t * 1000.0).round() as u64,
                words[..reveal_cursor].join(" "),
            ));
        }
        t += interim_sec;
    }

    if reveal_cursor < total_words {
        targets.push((
            (total_duration * 1000.0).round() as u64,
            words[..total_words].join(" "),
        ));
    }

    targets
}

fn preview_timing_steps(model: PreviewTimingModel) -> Vec<PreviewTimingStep> {
    let partial_targets = preview_partial_targets(PREVIEW_SAMPLE_TEXT, model.effective_interim_sec);
    let tick_ms = ((1000.0 / model.typing_cps as f64).round() as u64).max(1);
    let mut visible_text = String::new();
    let mut previous_target = String::new();
    let mut last_emit_done_ms = 0u64;
    let mut steps = Vec::new();

    for (index, (publish_ms, target_text)) in partial_targets.into_iter().enumerate() {
        let suffix = if target_text.starts_with(&previous_target) {
            target_text[previous_target.len()..].to_string()
        } else {
            target_text.clone()
        };

        if suffix.is_empty() {
            previous_target = target_text;
            continue;
        }

        let start_ms = if index == 0 {
            publish_ms
        } else {
            publish_ms
                .saturating_add(model.buffer_delay_ms)
                .max(last_emit_done_ms)
        };

        let mut current_ms = start_ms;
        for chunk in preview_emit_chunks(&suffix, model.emit_words_max) {
            visible_text.push_str(&chunk);
            steps.push(PreviewTimingStep {
                publish_ms,
                visible_at_ms: current_ms,
                target_text: target_text.clone(),
                visible_text: visible_text.clone(),
            });
            current_ms = current_ms.saturating_add(tick_ms);
        }

        last_emit_done_ms = current_ms;
        previous_target = target_text;
    }

    steps
}

fn preview_timing_summary_text(model: PreviewTimingModel) -> String {
    let tick_ms = ((1000.0 / model.typing_cps as f64).round() as u64).max(1);
    if model.overlay_enabled {
        format!(
            "Overlay ON • partial target every {:.1}s • first output immediate • later growth +{}ms • {} words/tick • {}ms between ticks",
            model.effective_interim_sec, model.buffer_delay_ms, model.emit_words_max, tick_ms
        )
    } else {
        format!(
            "Overlay OFF • runtime hides floating preview • cadence clamped to {:.1}s (requested {:.1}s) • if shown: +{}ms buffer • {} words/tick • {}ms ticks",
            model.effective_interim_sec,
            model.requested_interim_sec,
            model.buffer_delay_ms,
            model.emit_words_max,
            tick_ms
        )
    }
}

fn preview_timing_report_text(model: PreviewTimingModel) -> String {
    let partial_targets = preview_partial_targets(PREVIEW_SAMPLE_TEXT, model.effective_interim_sec);
    let steps = preview_timing_steps(model);
    let mut lines = Vec::new();
    lines.push(format!("Sample: {PREVIEW_SAMPLE_TEXT}"));
    lines.push(String::new());
    lines.push("Chunker partial targets".to_string());
    for (publish_ms, target_text) in partial_targets {
        lines.push(format!(
            "[{:.1}s] {}",
            publish_ms as f32 / 1000.0,
            target_text
        ));
    }
    lines.push(String::new());
    lines.push(if model.overlay_enabled {
        "Overlay-visible text".to_string()
    } else {
        "Overlay-visible text (would look like this if overlay was enabled)".to_string()
    });
    for step in steps {
        lines.push(format!(
            "[publish {:.1}s -> visible {:.2}s] {}",
            step.publish_ms as f32 / 1000.0,
            step.visible_at_ms as f32 / 1000.0,
            step.visible_text
        ));
    }
    lines.join("\n")
}

unsafe fn style_paper_input(field: Id) {
    let _: () = msg_send![field, setDrawsBackground: true];
    let input_bg = unsafe { settings_input_paper_bg() };
    let _: () = msg_send![field, setBackgroundColor: input_bg];
}

unsafe fn settings_input_paper_bg() -> Id {
    let base = ui_colors::surface_paper_warm();
    msg_send![base, colorWithAlphaComponent: 0.84f64]
}

unsafe fn add_tafla_header_separator(container: Id, x: f64, y: f64, width: f64) -> f64 {
    let separator = create_label(LabelConfig {
        frame: CGRect::new(&CGPoint::new(x, y), &CGSize::new(width, 1.0)),
        text: String::new(),
        background_color: Some(ui_colors::header_border()),
        ..Default::default()
    });
    let _: () = msg_send![separator, setAlphaValue: 0.9f64];
    unsafe {
        add_subview(container, separator);
    }
    y - 1.0
}

unsafe fn add_slider_setting_row(
    container: Id,
    action_handler: Id,
    x: f64,
    y: &mut f64,
    width: f64,
    secondary: Id,
    spec: SliderSettingRowSpec<'_>,
) -> usize {
    let label = create_label(LabelConfig {
        frame: CGRect::new(&CGPoint::new(x, *y), &CGSize::new(136.0, 18.0)),
        text: spec.title.to_string(),
        font_size: ui_tokens::SMALL_FONT_SIZE,
        text_color: secondary,
        ..Default::default()
    });
    unsafe {
        add_subview(container, label);
    }

    let value_label = create_label(LabelConfig {
        frame: CGRect::new(
            &CGPoint::new(x + width - 110.0, *y),
            &CGSize::new(110.0, 18.0),
        ),
        text: spec.value_text.to_string(),
        font_size: ui_tokens::SMALL_FONT_SIZE,
        text_color: secondary,
        ..Default::default()
    });
    unsafe {
        add_subview(container, value_label);
    }

    let slider = create_slider(
        CGRect::new(
            &CGPoint::new(x + 140.0, *y - 1.0),
            &CGSize::new((width - 254.0).max(160.0), 20.0),
        ),
        spec.min,
        spec.max,
        spec.current,
    );
    let _: () = msg_send![slider, setContinuous: true];
    unsafe {
        button_set_action(slider, action_handler, spec.action);
        add_subview(container, slider);
    }

    *y -= 24.0 + spec.gap;
    value_label as usize
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
    let ns_scroll_view = objc_class("NSScrollView");
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
    let text_width = (width - TOGGLE_SWITCH_WIDTH - 10.0).max(80.0);
    let title_label = create_label(LabelConfig {
        frame: CGRect::new(
            &CGPoint::new(x, *y + 1.0),
            &CGSize::new(text_width, TOGGLE_ROW_HEIGHT),
        ),
        text: spec.title.to_string(),
        font_size: ui_tokens::BODY_FONT_SIZE,
        text_color: crate::ui_helpers::color_label(),
        ..Default::default()
    });
    unsafe {
        add_subview(container, title_label);
    }

    let toggle = create_toggle(
        CGRect::new(
            &CGPoint::new(x + width - TOGGLE_SWITCH_WIDTH, *y),
            &CGSize::new(TOGGLE_SWITCH_WIDTH, TOGGLE_SWITCH_HEIGHT),
        ),
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
                    (width - TOGGLE_ROW_LABEL_INDENT - TOGGLE_SWITCH_WIDTH - 10.0).max(60.0),
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
    preview_typing_cps_value_label: Option<usize>,
    preview_emit_words_max_value_label: Option<usize>,
    preview_interim_sec_value_label: Option<usize>,
    preview_timing_summary_label: Option<usize>,
    preview_timing_text_view: Option<usize>,
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
    state.active_tab = TAB_TRANSCRIPTION;
    state.keys_mode_binding_labels = [None; 3];
    state.keys_recorder_hint_label = None;
    state.keys_conflict_label = None;
    state.keys_conflict_details_button = None;
    state.hold_delay_value_label = None;
    state.double_tap_value_label = None;
    state.preview_buffer_delay_value_label = None;
    state.preview_typing_cps_value_label = None;
    state.preview_emit_words_max_value_label = None;
    state.preview_interim_sec_value_label = None;
    state.preview_timing_summary_label = None;
    state.preview_timing_text_view = None;
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
        state.preview_typing_cps_value_label = built_state.preview_typing_cps_value_label;
        state.preview_emit_words_max_value_label = built_state.preview_emit_words_max_value_label;
        state.preview_interim_sec_value_label = built_state.preview_interim_sec_value_label;
        state.preview_timing_summary_label = built_state.preview_timing_summary_label;
        state.preview_timing_text_view = built_state.preview_timing_text_view;
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
        let mut state = SETTINGS_WINDOW_STATE
            .lock()
            .unwrap_or_else(|e| e.into_inner());
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
            reconcile_permission_runtime_after_grant(kind);
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

    if granted {
        reconcile_permission_runtime_after_grant(kind);
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

fn key_status_symbol_name(is_set: bool) -> &'static str {
    if is_set {
        "checkmark.seal.fill"
    } else {
        "circle"
    }
}

fn formatting_key_is_set() -> bool {
    keychain_key_is_set("LLM_FORMATTING_API_KEY")
}

unsafe fn update_key_status_indicator(indicator: Id, is_set: bool) {
    let _ =
        unsafe { crate::ui_helpers::set_button_symbol(indicator, key_status_symbol_name(is_set)) };
    let supports_tint: bool = msg_send![indicator, respondsToSelector: sel!(setContentTintColor:)];
    if supports_tint {
        let _: () = msg_send![indicator, setContentTintColor: key_status_color(is_set)];
    }
}

unsafe fn create_key_status_indicator(frame: CGRect, is_set: bool) -> Id {
    let ns_button = objc_class("NSButton");
    let indicator: Id = msg_send![ns_button, alloc];
    let indicator: Id = msg_send![indicator, initWithFrame: frame];
    let _: () = msg_send![indicator, setBordered: false];
    let _: () = msg_send![indicator, setEnabled: false];
    let _: () = msg_send![indicator, setTitle: ns_string("")];
    unsafe {
        update_key_status_indicator(indicator, is_set);
    }
    indicator
}

fn update_keychain_status_labels() {
    let (llm_icon, llm_label, assist_icon, assist_label) = {
        let state = SETTINGS_WINDOW_STATE
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        (
            state.llm_key_status_icon,
            state.llm_key_status_label,
            state.assistive_key_status_icon,
            state.assistive_key_status_label,
        )
    };
    unsafe {
        if let Some(ptr) = llm_icon {
            let is_set = formatting_key_is_set();
            update_key_status_indicator(ptr as Id, is_set);
        }
        if let Some(ptr) = llm_label {
            let is_set = formatting_key_is_set();
            let label = ptr as Id;
            set_text_field_string(label, key_status_text(is_set));
            let _: () = msg_send![label, setTextColor: key_status_color(is_set)];
        }
        if let Some(ptr) = assist_icon {
            let is_set = keychain_key_is_set("LLM_ASSISTIVE_API_KEY");
            update_key_status_indicator(ptr as Id, is_set);
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
        let mut state = SETTINGS_WINDOW_STATE
            .lock()
            .unwrap_or_else(|e| e.into_inner());
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
                let state = SETTINGS_WINDOW_STATE
                    .lock()
                    .unwrap_or_else(|e| e.into_inner());
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
        let (labels, action_buttons, requested) = {
            let state = SETTINGS_WINDOW_STATE
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            (
                state.permission_labels,
                state.permission_action_buttons,
                state.permission_requested,
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

        refresh_diagnostics_dashboard();
    });
}

unsafe fn build_settings_ui(
    root_view: Id,
    settings_width: f64,
    settings_height: f64,
    action_handler: Id,
    config: &Config,
) -> SettingsWindowState {
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
        let tab_names = [
            "Transcription",
            "Modes & Shortcuts",
            "AI & Prompts",
            "Audio & Input",
            "Diagnostics",
        ];
        let tab_sels = [
            sel!(onTabTranscription:),
            sel!(onTabModesShortcuts:),
            sel!(onTabAiPrompts:),
            sel!(onTabAudioInput:),
            sel!(onTabDiagnostics:),
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

            let tab_btn = create_sidebar_tab_button(btn_frame, name, i == TAB_TRANSCRIPTION);
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

        // --- Transcription tab (index 0) ---
        let transcription_view =
            build_quality_tab(action_handler, tab_document_frame, config, &mut state);
        let transcription_scroll =
            wrap_tab_content_in_scroll_view(tab_content_frame, transcription_view);
        add_subview(content_container, transcription_scroll);

        // --- Modes & Shortcuts tab (index 1) ---
        let keys_view =
            build_modes_shortcuts_tab(action_handler, tab_document_frame, config, &mut state);
        let keys_scroll = wrap_tab_content_in_scroll_view(tab_content_frame, keys_view);
        let _: () = msg_send![keys_scroll, setHidden: true];
        add_subview(content_container, keys_scroll);

        // --- AI & Prompts tab (index 2) ---
        let api_view = build_ai_prompts_tab(action_handler, tab_document_frame, config, &mut state);
        let api_scroll = wrap_tab_content_in_scroll_view(tab_content_frame, api_view);
        let _: () = msg_send![api_scroll, setHidden: true];
        add_subview(content_container, api_scroll);

        // --- Audio & Input tab (index 3) ---
        let audio_view = build_audio_input_tab(action_handler, tab_document_frame, config);
        let audio_scroll = wrap_tab_content_in_scroll_view(tab_content_frame, audio_view);
        let _: () = msg_send![audio_scroll, setHidden: true];
        add_subview(content_container, audio_scroll);

        // --- Diagnostics tab (index 4) ---
        let diagnostics_view =
            build_diagnostics_tab(action_handler, tab_document_frame, config, &mut state);
        let diagnostics_scroll =
            wrap_tab_content_in_scroll_view(tab_content_frame, diagnostics_view);
        let _: () = msg_send![diagnostics_scroll, setHidden: true];
        add_subview(content_container, diagnostics_scroll);

        // ====================================================================
        // Store state
        // ====================================================================
        state.tab_buttons = tab_buttons;
        state.content_views = [
            Some(transcription_scroll as usize),
            Some(keys_scroll as usize),
            Some(api_scroll as usize),
            Some(audio_scroll as usize),
            Some(diagnostics_scroll as usize),
        ];
        state.active_tab = TAB_TRANSCRIPTION;
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
            "Transcription" => "waveform",
            "Modes & Shortcuts" => "keyboard",
            "AI & Prompts" => "text.bubble",
            "Audio & Input" => "speaker.wave.2",
            "Diagnostics" => "stethoscope",
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

        if index == TAB_TRANSCRIPTION {
            refresh_quality_dashboard();
            refresh_transcription_preview_panel();
        } else if index == TAB_DIAGNOSTICS {
            refresh_diagnostics_dashboard();
        } else if index == TAB_AI_PROMPTS {
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

fn mode_from_tag(tag: isize) -> Option<WorkMode> {
    match tag {
        MODE_DICTATION_TAG => Some(WorkMode::Dictation),
        MODE_FORMATTING_TAG => Some(WorkMode::Formatting),
        MODE_ASSISTIVE_TAG => Some(WorkMode::Assistive),
        _ => None,
    }
}

fn mode_from_disable_tag(tag: isize) -> Option<WorkMode> {
    mode_from_tag(tag - MODE_DISABLE_TAG_OFFSET)
}

fn mode_from_double_ctrl_tag(tag: isize) -> bool {
    tag == MODE_DICTATION_DOUBLE_CTRL_TAG
}

fn mode_label_slot(mode: WorkMode) -> usize {
    match mode {
        WorkMode::Dictation => 0,
        WorkMode::Formatting => 1,
        WorkMode::Assistive => 2,
    }
}

fn set_mode_recorder_hint(text: &str, is_error: bool) {
    let hint_ptr = {
        let state = SETTINGS_WINDOW_STATE
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        state.keys_recorder_hint_label
    };
    let Some(hint_ptr) = hint_ptr else {
        return;
    };
    unsafe {
        let hint_label = hint_ptr as Id;
        set_text_field_string(hint_label, text);
        let color = if is_error {
            ui_colors::bubble_error_text()
        } else {
            crate::ui_helpers::color_secondary_label()
        };
        let _: () = msg_send![hint_label, setTextColor: color];
    }
}

fn refresh_mode_binding_labels() {
    let settings = UserSettings::load();
    let state = SETTINGS_WINDOW_STATE
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    for mode in [
        WorkMode::Dictation,
        WorkMode::Formatting,
        WorkMode::Assistive,
    ] {
        if let Some(label_ptr) = state.keys_mode_binding_labels[mode_label_slot(mode)] {
            let text = settings.mode_binding_for(mode).label().to_string();
            unsafe {
                set_text_field_string(label_ptr as Id, &text);
            }
        }
    }
}

fn binding_from_recorded_event(
    mode: WorkMode,
    event_type: u64,
    keycode: u16,
    flags: u64,
) -> Option<ShortcutBinding> {
    // NSEventModifierFlagShift/Control/Option/Command
    const SHIFT: u64 = 1 << 17;
    const CONTROL: u64 = 1 << 18;
    const OPTION: u64 = 1 << 19;
    const COMMAND: u64 = 1 << 20;
    const EVENT_TYPE_FLAGS_CHANGED: u64 = 12;

    match mode {
        WorkMode::Dictation => match keycode {
            63 => Some(ShortcutBinding::HoldFn),
            59 | 62 => {
                if (flags & OPTION) != 0 {
                    Some(ShortcutBinding::HoldCtrlAlt)
                } else if (flags & SHIFT) != 0 {
                    Some(ShortcutBinding::HoldCtrlShift)
                } else if (flags & COMMAND) != 0 {
                    Some(ShortcutBinding::HoldCtrlCmd)
                } else if event_type == EVENT_TYPE_FLAGS_CHANGED && (flags & CONTROL) != 0 {
                    Some(ShortcutBinding::HoldCtrl)
                } else {
                    None
                }
            }
            _ => None,
        },
        WorkMode::Formatting => match keycode {
            58 => Some(ShortcutBinding::DoubleLeftOption),
            _ => None,
        },
        WorkMode::Assistive => match keycode {
            63 => Some(ShortcutBinding::HoldFn),
            59 | 62 => {
                if (flags & OPTION) != 0 {
                    Some(ShortcutBinding::HoldCtrlAlt)
                } else if (flags & SHIFT) != 0 {
                    Some(ShortcutBinding::HoldCtrlShift)
                } else if (flags & COMMAND) != 0 {
                    Some(ShortcutBinding::HoldCtrlCmd)
                } else if event_type == EVENT_TYPE_FLAGS_CHANGED && (flags & CONTROL) != 0 {
                    Some(ShortcutBinding::HoldCtrl)
                } else {
                    None
                }
            }
            61 => Some(ShortcutBinding::DoubleRightOption),
            _ => None,
        },
    }
}

fn mode_accepts_binding(mode: WorkMode, binding: ShortcutBinding) -> bool {
    match mode {
        WorkMode::Dictation => matches!(
            binding,
            ShortcutBinding::Disabled
                | ShortcutBinding::HoldFn
                | ShortcutBinding::HoldCtrl
                | ShortcutBinding::HoldCtrlAlt
                | ShortcutBinding::HoldCtrlShift
                | ShortcutBinding::HoldCtrlCmd
                | ShortcutBinding::DoubleCtrl
        ),
        WorkMode::Formatting => {
            matches!(
                binding,
                ShortcutBinding::Disabled | ShortcutBinding::DoubleLeftOption
            )
        }
        WorkMode::Assistive => {
            matches!(
                binding,
                ShortcutBinding::Disabled
                    | ShortcutBinding::HoldFn
                    | ShortcutBinding::HoldCtrl
                    | ShortcutBinding::HoldCtrlAlt
                    | ShortcutBinding::HoldCtrlShift
                    | ShortcutBinding::HoldCtrlCmd
                    | ShortcutBinding::DoubleRightOption
            )
        }
    }
}

fn mode_binding_selection_error(
    mode: WorkMode,
    binding: ShortcutBinding,
    settings: &UserSettings,
) -> Option<String> {
    if !mode_accepts_binding(mode, binding) {
        return Some(format!(
            "{} mode supports only {} bindings.",
            mode.label(),
            match mode {
                WorkMode::Dictation => "hold modifiers or Double Ctrl",
                WorkMode::Formatting => "Double Left Option",
                WorkMode::Assistive => "hold modifiers or Double Right Option",
            }
        ));
    }

    if mode != WorkMode::Dictation
        && binding != ShortcutBinding::Disabled
        && settings.mode_binding_for(WorkMode::Dictation) == ShortcutBinding::DoubleCtrl
        && matches!(
            binding,
            ShortcutBinding::DoubleLeftOption | ShortcutBinding::DoubleRightOption
        )
    {
        return Some(
            "Dictation is currently on Double Ctrl. Disable it first to use Option bindings."
                .to_string(),
        );
    }

    None
}

fn apply_mode_binding(mode: WorkMode, binding: ShortcutBinding) {
    let mut settings = UserSettings::load();
    if let Some(message) = mode_binding_selection_error(mode, binding, &settings) {
        set_mode_recorder_hint(&message, true);
        return;
    }

    settings.set_mode_binding(mode, binding);

    if mode == WorkMode::Dictation && binding == ShortcutBinding::DoubleCtrl {
        settings.set_mode_binding(WorkMode::Formatting, ShortcutBinding::Disabled);
        settings.set_mode_binding(WorkMode::Assistive, ShortcutBinding::Disabled);
    }

    let config = Config::load();
    hotkeys::apply_hotkey_runtime_config(hotkeys::HotkeyRuntimeConfig::from(&config));
    sync_runtime_config_via_ipc();

    refresh_mode_binding_labels();
    refresh_hotkey_conflict_indicator();
    set_mode_recorder_hint(
        &format!("{} mode -> {}", mode.label(), binding.label()),
        false,
    );
}

fn apply_recorded_mode_binding(mode: WorkMode, binding: ShortcutBinding) {
    apply_mode_binding(mode, binding);
}

fn recorder_capture_mode() -> Option<WorkMode> {
    let recorder = MODE_BINDING_RECORDER_STATE
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    recorder.target_mode
}

fn recorder_clear_target_mode() {
    let mut recorder = MODE_BINDING_RECORDER_STATE
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    recorder.target_mode = None;
}

fn handle_mode_binding_recorder_event(event: Id) -> Id {
    let Some(mode) = recorder_capture_mode() else {
        return event;
    };

    unsafe {
        let event_type: u64 = msg_send![event, type];
        let keycode: u16 = msg_send![event, keyCode];
        let flags: u64 = msg_send![event, modifierFlags];

        // Escape cancels recording.
        if event_type == 10 && keycode == 53 {
            recorder_clear_target_mode();
            set_mode_recorder_hint("Mode binding capture cancelled.", false);
            return std::ptr::null_mut();
        }

        if let Some(binding) = binding_from_recorded_event(mode, event_type, keycode, flags) {
            recorder_clear_target_mode();
            apply_recorded_mode_binding(mode, binding);
            return std::ptr::null_mut();
        }
    }

    set_mode_recorder_hint(
        "Unsupported shortcut for this mode. Press Esc to cancel capture.",
        true,
    );
    std::ptr::null_mut()
}

fn ensure_mode_binding_recorder_monitor() -> bool {
    let should_install = {
        let recorder = MODE_BINDING_RECORDER_STATE
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        !recorder.monitor_installed
    };
    if !should_install {
        return true;
    }

    unsafe {
        let ns_event = objc_class("NSEvent");
        let mask: u64 = (1_u64 << 10) | (1_u64 << 12); // keyDown + flagsChanged
        let handler = block::ConcreteBlock::new(|event: Id| -> Id {
            handle_mode_binding_recorder_event(event)
        })
        .copy();
        let monitor: Id =
            msg_send![ns_event, addLocalMonitorForEventsMatchingMask: mask handler: &*handler];
        if monitor.is_null() {
            warn!("Mode binding recorder: failed to install local event monitor");
            return false;
        }
        std::mem::forget(handler);
    }

    let mut recorder = MODE_BINDING_RECORDER_STATE
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    recorder.monitor_installed = true;
    true
}

fn start_mode_binding_recorder(mode: WorkMode) {
    if !ensure_mode_binding_recorder_monitor() {
        set_mode_recorder_hint("Mode binding recorder failed to initialize.", true);
        return;
    }
    {
        let mut recorder = MODE_BINDING_RECORDER_STATE
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        recorder.target_mode = Some(mode);
    }
    set_mode_recorder_hint(
        &format!(
            "Recording {} binding... Press Fn/Ctrl/Option (Esc to cancel).",
            mode.label()
        ),
        false,
    );
}

fn hotkey_conflicts(_config: &Config) -> Vec<shortcut_registry::HotkeyConflict> {
    let settings = UserSettings::load();
    shortcut_registry::detect_hotkey_conflicts(&settings)
}

fn hotkey_conflict_status_from(conflicts: &[shortcut_registry::HotkeyConflict]) -> (String, bool) {
    if conflicts.is_empty() {
        return ("Mode shortcuts: clear.".to_string(), false);
    }

    let first = &conflicts[0];
    let extra = conflicts.len().saturating_sub(1);
    let suffix = if extra > 0 {
        format!(" (+{} more)", extra)
    } else {
        String::new()
    };

    (
        format!(
            "Review shortcut: {} -> {}{}",
            first.gesture.label(),
            first.message,
            suffix
        ),
        true,
    )
}

fn hotkey_conflict_status(config: &Config) -> (String, bool) {
    let conflicts = hotkey_conflicts(config);
    hotkey_conflict_status_from(&conflicts)
}

fn hotkey_conflict_details_text(conflicts: &[shortcut_registry::HotkeyConflict]) -> String {
    if conflicts.is_empty() {
        return "No conflicts detected in current mode shortcuts.".to_string();
    }

    let mut lines = vec![
        "CodeScribe detected shortcuts that may overlap current mode bindings:".to_string(),
        String::new(),
    ];
    for (index, conflict) in conflicts.iter().enumerate() {
        lines.push(format!(
            "{}. {} -> {}",
            index + 1,
            conflict.gesture.label(),
            conflict.message
        ));
    }
    lines.push(String::new());
    lines.push("Recommendation: change that mode binding only if the gesture does not behave correctly at runtime.".to_string());
    lines.join("\n")
}

fn set_hotkey_conflict_details_button_enabled(button_ptr: Option<usize>, enabled: bool) {
    let Some(button_ptr) = button_ptr else {
        return;
    };
    unsafe {
        let button = button_ptr as Id;
        let _: () = msg_send![button, setEnabled: enabled];
    }
}

fn refresh_hotkey_conflict_indicator() {
    let config = Config::load();
    let (label_ptr, button_ptr) = {
        let state = SETTINGS_WINDOW_STATE
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        (
            state.keys_conflict_label,
            state.keys_conflict_details_button,
        )
    };
    apply_hotkey_conflict_indicator(label_ptr, button_ptr, &config);
}

fn apply_hotkey_conflict_indicator(
    label_ptr: Option<usize>,
    button_ptr: Option<usize>,
    config: &Config,
) {
    let conflicts = hotkey_conflicts(config);
    let (text, has_conflict) = hotkey_conflict_status_from(&conflicts);
    set_hotkey_conflict_details_button_enabled(button_ptr, has_conflict);

    let Some(label_ptr) = label_ptr else {
        return;
    };
    unsafe {
        let label = label_ptr as Id;
        set_text_field_string(label, &text);
        let color = if has_conflict {
            ui_colors::bubble_error_text()
        } else {
            crate::ui_helpers::color_secondary_label()
        };
        let _: () = msg_send![label, setTextColor: color];
    }
}

fn show_hotkey_conflicts_sheet() {
    let config = Config::load();
    let conflicts = hotkey_conflicts(&config);
    let title = if conflicts.is_empty() {
        "No Shortcut Conflicts"
    } else {
        "Shortcut Conflicts Detected"
    };
    let details = hotkey_conflict_details_text(&conflicts);
    let window_ptr = {
        let state = SETTINGS_WINDOW_STATE
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        state.window
    };

    unsafe {
        let ns_alert = objc_class("NSAlert");
        let alert: Id = msg_send![ns_alert, new];
        let _: () = msg_send![alert, setMessageText: ns_string(title)];
        let _: () = msg_send![alert, setInformativeText: ns_string(&details)];
        let _: () = msg_send![alert, setAlertStyle: 1_isize]; // NSAlertStyleInformational
        let _: () = msg_send![alert, addButtonWithTitle: ns_string("OK")];

        if let Some(window_ptr) = window_ptr {
            let window = window_ptr as Id;
            if !window.is_null() {
                let nil: Id = std::ptr::null_mut();
                let _: () =
                    msg_send![alert, beginSheetModalForWindow: window completionHandler: nil];
                return;
            }
        }

        let _: isize = msg_send![alert, runModal];
    }
}

fn prompt_type_from_index(index: isize) -> &'static str {
    if index == 1 {
        "assistive"
    } else {
        "formatting"
    }
}

fn prompt_display_name(prompt_type: &str) -> &'static str {
    if prompt_type == "assistive" {
        "Assistive"
    } else {
        "Formatting"
    }
}

fn selected_prompt_type() -> &'static str {
    let popup_ptr = {
        let state = SETTINGS_WINDOW_STATE
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        state.prompt_type_popup
    };
    let Some(popup_ptr) = popup_ptr else {
        return "formatting";
    };
    unsafe {
        let popup = popup_ptr as Id;
        let idx: isize = msg_send![popup, indexOfSelectedItem];
        prompt_type_from_index(idx)
    }
}

fn prompt_path_text(prompt_type: &str) -> String {
    if prompt_type == "assistive" {
        crate::get_assistive_prompt_path().display().to_string()
    } else {
        crate::get_formatting_prompt_path().display().to_string()
    }
}

fn load_prompt_content(prompt_type: &str) -> Result<String, String> {
    match send_ipc(IpcCommand::GetPrompt {
        prompt_type: prompt_type.to_string(),
    }) {
        Ok(IpcResponse::Prompt(content)) => Ok(content),
        Ok(IpcResponse::Error(err)) => Err(err),
        Ok(other) => Err(format!("Unexpected IPC response: {other:?}")),
        Err(err) => {
            warn!("Settings: prompt IPC unavailable, using config fallback: {err}");
            Ok(if prompt_type == "assistive" {
                crate::config::get_assistive_prompt()
            } else {
                crate::config::get_formatting_prompt()
            })
        }
    }
}

fn save_prompt_content(prompt_type: &str, content: &str) -> Result<(), String> {
    match send_ipc(IpcCommand::SavePrompt {
        prompt_type: prompt_type.to_string(),
        content: content.to_string(),
    }) {
        Ok(IpcResponse::Ok) => Ok(()),
        Ok(IpcResponse::Error(err)) => Err(err),
        Ok(other) => Err(format!("Unexpected IPC response: {other:?}")),
        Err(err) => {
            warn!("Settings: prompt IPC unavailable, using config fallback: {err}");
            let path = if prompt_type == "assistive" {
                crate::config::get_assistive_prompt_path()
            } else {
                crate::config::get_formatting_prompt_path()
            };
            if let Some(parent) = path.parent()
                && let Err(e) = fs::create_dir_all(parent)
            {
                return Err(e.to_string());
            }
            fs::write(path, content).map_err(|e| e.to_string())
        }
    }
}

fn reset_prompt_content(prompt_type: &str) -> Result<(), String> {
    match send_ipc(IpcCommand::ResetPrompt {
        prompt_type: prompt_type.to_string(),
    }) {
        Ok(IpcResponse::Ok) => Ok(()),
        Ok(IpcResponse::Error(err)) => Err(err),
        Ok(other) => Err(format!("Unexpected IPC response: {other:?}")),
        Err(err) => {
            warn!("Settings: prompt IPC unavailable, using config fallback: {err}");
            let path = if prompt_type == "assistive" {
                crate::config::get_assistive_prompt_path()
            } else {
                crate::config::get_formatting_prompt_path()
            };
            let default = if prompt_type == "assistive" {
                crate::config::DEFAULT_ASSISTIVE_PROMPT
            } else {
                crate::config::DEFAULT_FORMATTING_PROMPT
            };
            if let Some(parent) = path.parent()
                && let Err(e) = fs::create_dir_all(parent)
            {
                return Err(e.to_string());
            }
            fs::write(path, default).map_err(|e| e.to_string())
        }
    }
}

fn set_prompt_editor_content(text: &str) {
    let text_view_ptr = {
        let state = SETTINGS_WINDOW_STATE
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        state.prompt_editor_text_view
    };
    let Some(text_view_ptr) = text_view_ptr else {
        return;
    };
    unsafe {
        set_text_view_string(text_view_ptr as Id, text);
    }
}

fn read_prompt_editor_content() -> String {
    let text_view_ptr = {
        let state = SETTINGS_WINDOW_STATE
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        state.prompt_editor_text_view
    };
    let Some(text_view_ptr) = text_view_ptr else {
        return String::new();
    };
    unsafe { get_text_view_string(text_view_ptr as Id) }
}

fn set_prompt_editor_status(text: &str, is_error: bool) {
    let status_ptr = {
        let state = SETTINGS_WINDOW_STATE
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        state.prompt_status_label
    };
    let Some(status_ptr) = status_ptr else {
        return;
    };
    unsafe {
        let label = status_ptr as Id;
        set_text_field_string(label, text);
        let color = if is_error {
            ui_colors::bubble_error_text()
        } else {
            crate::ui_helpers::color_secondary_label()
        };
        let _: () = msg_send![label, setTextColor: color];
    }
}

fn refresh_transcription_preview_panel() {
    let model = current_preview_timing_model();
    let preview_text = preview_timing_report_text(model);
    let summary_text = preview_timing_summary_text(model);
    let (
        buffer_delay_label,
        typing_cps_label,
        emit_words_label,
        interim_label,
        summary_label,
        preview_text_view,
    ) = {
        let state = SETTINGS_WINDOW_STATE
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        (
            state.preview_buffer_delay_value_label,
            state.preview_typing_cps_value_label,
            state.preview_emit_words_max_value_label,
            state.preview_interim_sec_value_label,
            state.preview_timing_summary_label,
            state.preview_timing_text_view,
        )
    };

    unsafe {
        if let Some(ptr) = buffer_delay_label {
            set_text_field_string(ptr as Id, &format!("{} ms", model.buffer_delay_ms));
        }
        if let Some(ptr) = typing_cps_label {
            set_text_field_string(ptr as Id, &format!("{:.1} cps", model.typing_cps));
        }
        if let Some(ptr) = emit_words_label {
            set_text_field_string(ptr as Id, &format!("{} words", model.emit_words_max));
        }
        if let Some(ptr) = interim_label {
            let label = if (model.effective_interim_sec - model.requested_interim_sec).abs()
                > f32::EPSILON
            {
                format!(
                    "{:.1} s -> {:.1} s effective",
                    model.requested_interim_sec, model.effective_interim_sec
                )
            } else {
                format!("{:.1} s", model.effective_interim_sec)
            };
            set_text_field_string(ptr as Id, &label);
        }
        if let Some(ptr) = summary_label {
            set_text_field_string(ptr as Id, &summary_text);
        }
        if let Some(ptr) = preview_text_view {
            set_text_view_string(ptr as Id, &preview_text);
        }
    }
}

fn refresh_prompt_editor_labels() {
    Queue::main().exec_async(move || unsafe {
        let (path_ptr, status_ptr) = {
            let state = SETTINGS_WINDOW_STATE
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            (state.prompt_path_label, state.prompt_status_label)
        };
        let prompt_type = selected_prompt_type();
        if let Some(ptr) = path_ptr {
            let path_text = format!("Path: {}", prompt_path_text(prompt_type));
            set_text_field_string(ptr as Id, &path_text);
        }
        if let Some(ptr) = status_ptr {
            let hint = if prompt_type == "assistive" {
                "Editing assistive prompt."
            } else {
                "Editing formatting prompt."
            };
            set_text_field_string(ptr as Id, hint);
            let _: () =
                msg_send![ptr as Id, setTextColor: crate::ui_helpers::color_secondary_label()];
        }
    });
}

#[derive(Clone, Copy, Debug)]
struct PromptEditorLayout {
    editor_height: f64,
    editor_y: f64,
    status_y: f64,
}

fn compute_prompt_editor_layout(y: f64, gap: f64) -> PromptEditorLayout {
    // Keep status text and bottom breathing room below the editor so the editor
    // never climbs into API/model/key controls on smaller vertical space.
    let reserved_below_editor = PROMPT_EDITOR_STATUS_HEIGHT + gap + PROMPT_EDITOR_BOTTOM_PADDING;
    let available_editor_height = (y - reserved_below_editor).max(0.0);
    let editor_height = available_editor_height.min(PROMPT_EDITOR_DESIRED_HEIGHT);
    let editor_y = (y - editor_height).max(0.0);
    let status_y = (editor_y - gap).max(0.0);

    PromptEditorLayout {
        editor_height,
        editor_y,
        status_y,
    }
}

fn refresh_quality_dashboard() {
    Queue::main().exec_async(move || unsafe {
        let (available_label, pending_label, last_check_label, report_label, open_report_button) = {
            let state = SETTINGS_WINDOW_STATE
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            (
                state.quality_available_label,
                state.quality_pending_label,
                state.quality_last_check_label,
                state.qube_report_label,
                state.quality_open_report_button,
            )
        };

        let snapshot = crate::qube_lifecycle::dashboard_snapshot();
        let daemon_state = &snapshot.daemon_state;

        if let Some(ptr) = available_label {
            let label = ptr as Id;
            set_text_field_string(label, snapshot.availability_label());
            let _: () = msg_send![
                label,
                setTextColor: if snapshot.available {
                    ui_colors::status_granted()
                } else {
                    ui_colors::status_warning()
                }
            ];
        }

        if let Some(ptr) = pending_label {
            let label = ptr as Id;
            set_text_field_string(label, &daemon_state.pending_mismatches.to_string());
            let _: () = msg_send![
                label,
                setTextColor: if daemon_state.pending_mismatches > 0 {
                    ui_colors::status_warning()
                } else {
                    crate::ui_helpers::color_secondary_label()
                }
            ];
        }

        if let Some(ptr) = last_check_label {
            set_text_field_string(
                ptr as Id,
                &quality_last_check_text(&daemon_state.last_check),
            );
        }

        if let Some(ptr) = report_label {
            set_text_field_string(ptr as Id, &qube_report_text(daemon_state));
        }

        if let Some(ptr) = open_report_button {
            let _: () = msg_send![ptr as Id, setEnabled: qube_report_exists(daemon_state)];
        }
    });
}

fn refresh_diagnostics_dashboard() {
    Queue::main().exec_async(move || unsafe {
        let (permission_labels, conflict_label, conflict_button, status_label) = {
            let state = SETTINGS_WINDOW_STATE
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            (
                state.diagnostics_permission_labels,
                state.diagnostics_conflict_label,
                state.diagnostics_conflict_details_button,
                state.diagnostics_status_label,
            )
        };

        for kind in PERMISSION_ORDER {
            let idx = kind.index();
            let status = permission_status(kind);
            if let Some(ptr) = permission_labels[idx] {
                let label = ptr as Id;
                set_text_field_string(label, permission_status_text(status));
                let _: () = msg_send![label, setTextColor: permission_status_color(status)];
            }
        }

        let config = Config::load();
        apply_hotkey_conflict_indicator(conflict_label, conflict_button, &config);

        if let Some(ptr) = status_label {
            set_text_field_string(
                ptr as Id,
                "Use Copy diagnostics to capture a full environment + permission report.",
            );
        }
    });
}

// ============================================================================
// Modes & Shortcuts tab
// ============================================================================

unsafe fn build_modes_shortcuts_tab(
    action_handler: Id,
    frame: core_graphics::geometry::CGRect,
    config: &Config,
    state: &mut SettingsWindowState,
) -> Id {
    use core_graphics::geometry::{CGPoint, CGRect, CGSize};
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

unsafe fn build_ai_prompts_tab(
    action_handler: Id,
    frame: core_graphics::geometry::CGRect,
    _config: &Config,
    state: &mut SettingsWindowState,
) -> Id {
    use core_graphics::geometry::{CGPoint, CGRect, CGSize};
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
        let mono_font_input = crate::ui_helpers::monospace_font(ui_tokens::BODY_FONT_SIZE);

        let title = create_label(LabelConfig {
            frame: CGRect::new(&CGPoint::new(pad, y), &CGSize::new(content_w, 24.0)),
            text: "AI & Prompts".to_string(),
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
            text: "Runtime AI endpoints plus an in-app prompt editor for formatting + assistive (agent) modes."
                .to_string(),
            font_size: ui_tokens::MICRO_FONT_SIZE,
            text_color: secondary,
            ..Default::default()
        });
        add_subview(container, subtitle);
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

        let llm_endpoint_val = std::env::var("LLM_FORMATTING_ENDPOINT")
            .ok()
            .filter(|v| !v.trim().is_empty())
            .unwrap_or_default();
        let llm_endpoint_field = create_text_input(
            CGRect::new(
                &CGPoint::new(pad, y),
                &CGSize::new(content_w, SETTINGS_INPUT_HEIGHT),
            ),
            "Endpoint (e.g. https://api.libraxis.cloud/v1/responses)",
            &llm_endpoint_val,
        );
        style_paper_input(llm_endpoint_field);
        let _: () = msg_send![llm_endpoint_field, setFont: mono_font_input];
        button_set_action(
            llm_endpoint_field,
            action_handler,
            sel!(onLlmEndpointChanged:),
        );
        add_subview(container, llm_endpoint_field);
        state.llm_endpoint_field = Some(llm_endpoint_field as usize);
        y -= SETTINGS_INPUT_HEIGHT + gap;

        let llm_model_val = std::env::var("LLM_FORMATTING_MODEL")
            .ok()
            .filter(|v| !v.trim().is_empty())
            .unwrap_or_default();
        let llm_model_field = create_text_input(
            CGRect::new(
                &CGPoint::new(pad, y),
                &CGSize::new(content_w, SETTINGS_INPUT_HEIGHT),
            ),
            "Model (e.g. programmer)",
            &llm_model_val,
        );
        style_paper_input(llm_model_field);
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
        style_paper_input(llm_key_field);
        let _: () = msg_send![llm_key_field, setFont: mono_font_input];
        button_set_action(llm_key_field, action_handler, sel!(onLlmKeyChanged:));
        add_subview(container, llm_key_field);
        state.llm_key_field = Some(llm_key_field as usize);
        y -= SETTINGS_INPUT_HEIGHT + gap;

        let llm_key_status = formatting_key_is_set();
        let llm_status_icon = create_key_status_indicator(
            CGRect::new(
                &CGPoint::new(pad, y + 1.0),
                &CGSize::new(KEY_STATUS_ICON_SIZE, KEY_STATUS_ICON_SIZE),
            ),
            llm_key_status,
        );
        add_subview(container, llm_status_icon);
        state.llm_key_status_icon = Some(llm_status_icon as usize);

        let llm_status_label = create_label(LabelConfig {
            frame: CGRect::new(
                &CGPoint::new(pad + KEY_STATUS_ICON_SIZE + 6.0, y),
                &CGSize::new(content_w - KEY_STATUS_ICON_SIZE - 6.0, 16.0),
            ),
            text: key_status_text(llm_key_status).to_string(),
            font_size: ui_tokens::MICRO_FONT_SIZE,
            text_color: key_status_color(llm_key_status),
            ..Default::default()
        });
        add_subview(container, llm_status_label);
        state.llm_key_status_label = Some(llm_status_label as usize);
        y -= 16.0 + gap;

        // Section divider + extra gap before Assistive AI section
        y = add_tafla_header_separator(container, pad, y, content_w);
        y -= ui_tokens::SECTION_GAP;

        let assist_header = create_label(LabelConfig {
            frame: CGRect::new(&CGPoint::new(pad, y), &CGSize::new(content_w, 18.0)),
            text: "Assistive AI (agent chat)".to_string(),
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
        style_paper_input(assist_endpoint_field);
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
        style_paper_input(assist_model_field);
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
        style_paper_input(assist_key_field);
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
        let assist_status_icon = create_key_status_indicator(
            CGRect::new(
                &CGPoint::new(pad, y + 1.0),
                &CGSize::new(KEY_STATUS_ICON_SIZE, KEY_STATUS_ICON_SIZE),
            ),
            assist_key_status,
        );
        add_subview(container, assist_status_icon);
        state.assistive_key_status_icon = Some(assist_status_icon as usize);

        let assist_status_label = create_label(LabelConfig {
            frame: CGRect::new(
                &CGPoint::new(pad + KEY_STATUS_ICON_SIZE + 6.0, y),
                &CGSize::new(content_w - KEY_STATUS_ICON_SIZE - 6.0, 16.0),
            ),
            text: key_status_text(assist_key_status).to_string(),
            font_size: ui_tokens::MICRO_FONT_SIZE,
            text_color: key_status_color(assist_key_status),
            ..Default::default()
        });
        add_subview(container, assist_status_label);
        state.assistive_key_status_label = Some(assist_status_label as usize);
        y -= 16.0 + gap;

        let save_btn = create_button(
            CGRect::new(
                &CGPoint::new(frame.size.width - pad - 90.0, y - 2.0),
                &CGSize::new(90.0, 24.0),
            ),
            "Save AI",
            button_style::GLASS,
        );
        button_set_action(save_btn, action_handler, sel!(onSaveApiSettings:));
        add_subview(container, save_btn);
        y -= 24.0 + gap;

        let runtime_hint = create_label(LabelConfig {
            frame: CGRect::new(&CGPoint::new(pad, y), &CGSize::new(content_w - 98.0, 16.0)),
            text: "Save AI persists endpoint/model/key values. Prompt content is edited below."
                .to_string(),
            font_size: ui_tokens::MICRO_FONT_SIZE,
            text_color: secondary,
            ..Default::default()
        });
        add_subview(container, runtime_hint);
        y -= 16.0 + gap;

        let section_divider = create_label(LabelConfig {
            frame: CGRect::new(&CGPoint::new(pad, y), &CGSize::new(content_w, 1.0)),
            text: String::new(),
            background_color: Some(ui_colors::surface_border()),
            ..Default::default()
        });
        let _: () = msg_send![section_divider, setAlphaValue: 0.9f64];
        add_subview(container, section_divider);
        y -= ui_tokens::SECTION_GAP;

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

unsafe fn build_audio_input_tab(
    action_handler: Id,
    frame: core_graphics::geometry::CGRect,
    config: &Config,
) -> Id {
    use core_graphics::geometry::{CGPoint, CGRect, CGSize};
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

        // Section title
        let title = create_label(LabelConfig {
            frame: CGRect::new(&CGPoint::new(pad, y), &CGSize::new(content_w, 24.0)),
            text: "Audio & Input".to_string(),
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
            text: "Speech capture defaults, recorder feedback, and overlay behavior.".to_string(),
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
        let _overlay_check = add_toggle_row(
            container,
            action_handler,
            pad,
            &mut y,
            content_w,
            secondary,
            ToggleRowSpec {
                title: "Transcription overlay",
                checked: config.transcription_overlay_enabled,
                action: sel!(onTranscriptionOverlayToggled:),
                description: Some(
                    "On: live floating preview with fast partials. Off: no overlay and buffered partials for lower local load.",
                ),
                tag: None,
                gap,
            },
        );
        let _dock_check = add_toggle_row(
            container,
            action_handler,
            pad,
            &mut y,
            content_w,
            secondary,
            ToggleRowSpec {
                title: "Show Dock icon",
                checked: config.show_dock_icon,
                action: sel!(onShowDockIconToggled:),
                description: Some("Keep CodeScribe in the Dock after windows close."),
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

// ============================================================================
// Transcription tab
// ============================================================================

fn quality_last_check_text(last_check: &str) -> String {
    let trimmed = last_check.trim();
    if trimmed.is_empty() {
        "Never".to_string()
    } else {
        trimmed.to_string()
    }
}

fn qube_report_exists(state: &crate::qube_daemon::QubeDaemonState) -> bool {
    state
        .latest_report
        .as_ref()
        .map(|dir| PathBuf::from(dir).join("index.html").exists())
        .unwrap_or(false)
}

fn qube_report_text(state: &crate::qube_daemon::QubeDaemonState) -> String {
    match state.latest_report.as_ref() {
        Some(dir) => {
            let html_path = PathBuf::from(dir).join("index.html");
            if html_path.exists() {
                html_path.display().to_string()
            } else {
                format!("{dir} (missing index.html)")
            }
        }
        None => "(none)".to_string(),
    }
}

unsafe fn build_quality_tab(
    action_handler: Id,
    frame: core_graphics::geometry::CGRect,
    _config: &Config,
    state: &mut SettingsWindowState,
) -> Id {
    use core_graphics::geometry::{CGPoint, CGRect, CGSize};
    unsafe {
        let ns_view = objc_class("NSView");
        let container: Id = msg_send![ns_view, alloc];
        let container: Id = msg_send![container, initWithFrame: frame];

        let pad = ui_tokens::EDGE_PADDING;
        let content_w = frame.size.width - pad * 2.0;
        let field_w = content_w;
        let gap = ui_tokens::DENSITY_COMFORTABLE;
        let mut y = frame.size.height - (24.0 + gap);
        let mono_font_input = crate::ui_helpers::monospace_font(ui_tokens::BODY_FONT_SIZE);
        let primary = crate::ui_helpers::color_label();
        let secondary = crate::ui_helpers::color_secondary_label();
        let preview_model = current_preview_timing_model();

        let title = create_label(LabelConfig {
            frame: CGRect::new(&CGPoint::new(pad, y), &CGSize::new(content_w, 24.0)),
            text: "Transcription".to_string(),
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
            text: "Choose the backend for the committed transcript. Overlay preview stays local and provisional."
                .to_string(),
            font_size: ui_tokens::MICRO_FONT_SIZE,
            text_color: secondary,
            ..Default::default()
        });
        add_subview(container, subtitle);
        y -= 16.0 + gap;

        let engine_header = create_label(LabelConfig {
            frame: CGRect::new(&CGPoint::new(pad, y), &CGSize::new(content_w, 18.0)),
            text: "Final Transcript Path".to_string(),
            font_size: ui_tokens::SMALL_FONT_SIZE,
            bold: true,
            text_color: primary,
            ..Default::default()
        });
        add_subview(container, engine_header);
        y -= 18.0 + gap;

        let provider_label = create_label(LabelConfig {
            frame: CGRect::new(&CGPoint::new(pad, y), &CGSize::new(92.0, 18.0)),
            text: "Commit with:".to_string(),
            font_size: ui_tokens::SMALL_FONT_SIZE,
            text_color: secondary,
            ..Default::default()
        });
        add_subview(container, provider_label);

        let ns_popup = objc_class("NSPopUpButton");
        let provider_popup: Id = msg_send![ns_popup, alloc];
        let provider_popup: Id = msg_send![provider_popup, initWithFrame:
            CGRect::new(&CGPoint::new(pad + 96.0, y - 2.0), &CGSize::new(220.0, 24.0))
            pullsDown: false
        ];
        let _: () = msg_send![provider_popup, addItemWithTitle: ns_string("Local final verdict")];
        let _: () = msg_send![provider_popup, addItemWithTitle: ns_string("Cloud final verdict")];
        let provider_index: isize = if _config.use_local_stt { 0 } else { 1 };
        let _: () = msg_send![provider_popup, selectItemAtIndex: provider_index];
        button_set_action(provider_popup, action_handler, sel!(onSttProviderChanged:));
        add_subview(container, provider_popup);
        y -= 24.0 + gap;

        let backend_note = if _config.use_local_stt {
            "Current mode: preview stays local, then the local verdict becomes the committed transcript. File-based final pass can strengthen that verdict or surface weak-truth warnings before paste/save."
        } else {
            "Current mode: preview stays local, then cloud STT becomes the committed verdict. If cloud does not return a reliable result, the app marks any surviving fallback as degraded and blocks silent auto-paste."
        };
        let provider_hint = create_label(LabelConfig {
            frame: CGRect::new(&CGPoint::new(pad, y), &CGSize::new(content_w, 16.0)),
            text: format!(
                "{backend_note} Endpoints ending with :stream use NDJSON and fit long buffered runs better."
            ),
            font_size: ui_tokens::MICRO_FONT_SIZE,
            text_color: secondary,
            ..Default::default()
        });
        add_subview(container, provider_hint);
        y -= 16.0 + gap;

        let stt_endpoint_val = std::env::var("STT_ENDPOINT").unwrap_or_default();
        let stt_endpoint_field = create_text_input(
            CGRect::new(
                &CGPoint::new(pad, y),
                &CGSize::new(content_w, SETTINGS_INPUT_HEIGHT),
            ),
            "Cloud final-verdict endpoint (multipart or ...:stream for NDJSON)",
            &stt_endpoint_val,
        );
        style_paper_input(stt_endpoint_field);
        let _: () = msg_send![stt_endpoint_field, setFont: mono_font_input];
        button_set_action(
            stt_endpoint_field,
            action_handler,
            sel!(onSttEndpointChanged:),
        );
        add_subview(container, stt_endpoint_field);
        y -= SETTINGS_INPUT_HEIGHT + gap;

        let stt_key_field = create_secure_text_input(
            CGRect::new(
                &CGPoint::new(pad, y),
                &CGSize::new(content_w, SETTINGS_INPUT_HEIGHT),
            ),
            "Cloud final-verdict API key (stored in Keychain; erase field to remove)",
        );
        style_paper_input(stt_key_field);
        let _: () = msg_send![stt_key_field, setFont: mono_font_input];
        button_set_action(stt_key_field, action_handler, sel!(onSttKeyChanged:));
        add_subview(container, stt_key_field);
        y -= SETTINGS_INPUT_HEIGHT + gap;

        y = add_tafla_header_separator(container, pad, y, content_w);
        y -= ui_tokens::SECTION_GAP;

        let preview_header = create_label(LabelConfig {
            frame: CGRect::new(&CGPoint::new(pad, y), &CGSize::new(content_w, 18.0)),
            text: "Preview Timing".to_string(),
            font_size: ui_tokens::SMALL_FONT_SIZE,
            bold: true,
            text_color: primary,
            ..Default::default()
        });
        add_subview(container, preview_header);
        y -= 18.0 + gap;

        let preview_hint = create_label(LabelConfig {
            frame: CGRect::new(&CGPoint::new(pad, y), &CGSize::new(content_w, 16.0)),
            text: "Live preview cadence. Buffer delay applies after the first visible partial."
                .to_string(),
            font_size: ui_tokens::MICRO_FONT_SIZE,
            text_color: secondary,
            ..Default::default()
        });
        add_subview(container, preview_hint);
        y -= 16.0 + gap;

        state.preview_buffer_delay_value_label = Some(add_slider_setting_row(
            container,
            action_handler,
            pad,
            &mut y,
            content_w,
            secondary,
            SliderSettingRowSpec {
                title: "Buffer delay:",
                value_text: &format!("{} ms", preview_model.buffer_delay_ms),
                min: 0.0,
                max: 1500.0,
                current: preview_model.buffer_delay_ms as f64,
                action: sel!(onPreviewBufferDelayChanged:),
                gap,
            },
        ));
        state.preview_typing_cps_value_label = Some(add_slider_setting_row(
            container,
            action_handler,
            pad,
            &mut y,
            content_w,
            secondary,
            SliderSettingRowSpec {
                title: "Typing speed:",
                value_text: &format!("{:.1} cps", preview_model.typing_cps),
                min: 5.0,
                max: 180.0,
                current: preview_model.typing_cps as f64,
                action: sel!(onPreviewTypingCpsChanged:),
                gap,
            },
        ));
        state.preview_emit_words_max_value_label = Some(add_slider_setting_row(
            container,
            action_handler,
            pad,
            &mut y,
            content_w,
            secondary,
            SliderSettingRowSpec {
                title: "Words per tick:",
                value_text: &format!("{} words", preview_model.emit_words_max),
                min: 1.0,
                max: 10.0,
                current: preview_model.emit_words_max as f64,
                action: sel!(onPreviewEmitWordsMaxChanged:),
                gap,
            },
        ));
        state.preview_interim_sec_value_label = Some(add_slider_setting_row(
            container,
            action_handler,
            pad,
            &mut y,
            content_w,
            secondary,
            SliderSettingRowSpec {
                title: "Interim cadence:",
                value_text: &if (preview_model.effective_interim_sec
                    - preview_model.requested_interim_sec)
                    .abs()
                    > f32::EPSILON
                {
                    format!(
                        "{:.1} s -> {:.1} s effective",
                        preview_model.requested_interim_sec, preview_model.effective_interim_sec
                    )
                } else {
                    format!("{:.1} s", preview_model.effective_interim_sec)
                },
                min: 1.0,
                max: 12.0,
                current: preview_model.requested_interim_sec as f64,
                action: sel!(onPreviewInterimCadenceChanged:),
                gap,
            },
        ));

        let preview_summary = create_label(LabelConfig {
            frame: CGRect::new(&CGPoint::new(pad, y), &CGSize::new(content_w, 16.0)),
            text: preview_timing_summary_text(preview_model),
            font_size: ui_tokens::MICRO_FONT_SIZE,
            text_color: secondary,
            ..Default::default()
        });
        add_subview(container, preview_summary);
        state.preview_timing_summary_label = Some(preview_summary as usize);
        y -= 16.0 + gap;

        let preview_box_height = 168.0;
        let (preview_scroll, preview_text_view) = create_scrollable_text_view(
            CGRect::new(
                &CGPoint::new(pad, y - preview_box_height),
                &CGSize::new(content_w, preview_box_height),
            ),
            false,
        );
        let _: () = msg_send![preview_text_view, setFont: mono_font_input];
        set_text_view_string(
            preview_text_view,
            &preview_timing_report_text(preview_model),
        );
        add_subview(container, preview_scroll);
        state.preview_timing_text_view = Some(preview_text_view as usize);
        y -= preview_box_height + gap;

        y = add_tafla_header_separator(container, pad, y, content_w);
        y -= ui_tokens::SECTION_GAP;

        let final_header = create_label(LabelConfig {
            frame: CGRect::new(&CGPoint::new(pad, y), &CGSize::new(content_w, 18.0)),
            text: "Final Transcript".to_string(),
            font_size: ui_tokens::SMALL_FONT_SIZE,
            bold: true,
            text_color: primary,
            ..Default::default()
        });
        add_subview(container, final_header);
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
                title: "Local file-based final pass",
                checked: ultra_on,
                action: sel!(onUltraQualityToggled:),
                description: Some(
                    "Re-runs local Whisper on the saved audio after capture ends. Best lever when preview looks fine but the committed verdict should be upgraded, downgraded, or blocked before paste/save.",
                ),
                tag: None,
                gap,
            },
        );
        state.ultra_quality_checkbox = Some(ultra_check as usize);

        let _fmt_check = add_toggle_row(
            container,
            action_handler,
            pad,
            &mut y,
            field_w,
            secondary,
            ToggleRowSpec {
                title: "AI Formatting",
                checked: _config.ai_formatting_enabled,
                action: sel!(onFormattingToggled:),
                description: Some(
                    "Uses the formatting model to clean up the committed transcript. If formatting fails, the raw transcript is preserved and labeled as a formatting fallback.",
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

        let ns_popup = objc_class("NSPopUpButton");
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

        let routing_hint = create_label(LabelConfig {
            frame: CGRect::new(&CGPoint::new(pad, y), &CGSize::new(content_w, 16.0)),
            text: "Whisper language lives in Audio & Input. AI endpoints and prompts live in AI & Prompts."
                .to_string(),
            font_size: ui_tokens::MICRO_FONT_SIZE,
            text_color: secondary,
            ..Default::default()
        });
        add_subview(container, routing_hint);
        y -= 16.0 + gap;

        let divider = create_label(LabelConfig {
            frame: CGRect::new(&CGPoint::new(pad, y), &CGSize::new(field_w, 1.0)),
            text: String::new(),
            background_color: Some(ui_colors::surface_border()),
            ..Default::default()
        });
        let _: () = msg_send![divider, setAlphaValue: 0.9f64];
        add_subview(container, divider);
        y -= ui_tokens::SECTION_GAP;

        let automation_header = create_label(LabelConfig {
            frame: CGRect::new(&CGPoint::new(pad, y), &CGSize::new(content_w, 18.0)),
            text: "Quality Automation".to_string(),
            font_size: ui_tokens::SMALL_FONT_SIZE,
            bold: true,
            text_color: primary,
            ..Default::default()
        });
        add_subview(container, automation_header);
        y -= 18.0 + gap;

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
            field_w,
            secondary,
            ToggleRowSpec {
                title: "Start quality daemon automatically",
                checked: quality_on,
                action: sel!(onQubeDaemonToggled:),
                description: Some(
                    "Starts bundled `qube-daemon --daemon` immediately and on next CodeScribe launch when the binary is installed. \
                     Turning it off stops the daemon only when CodeScribe owns that process; externally managed launchd or shell runs remain untouched.",
                ),
                tag: None,
                gap,
            },
        );
        state.qube_daemon_checkbox = Some(quality_check as usize);

        let automation_hint = create_label(LabelConfig {
            frame: CGRect::new(&CGPoint::new(pad, y), &CGSize::new(content_w, 16.0)),
            text: "Use reports below to compare raw, postprocess, and final output when the runtime starts lying."
                .to_string(),
            font_size: ui_tokens::MICRO_FONT_SIZE,
            text_color: secondary,
            ..Default::default()
        });
        add_subview(container, automation_hint);
        y -= 16.0 + gap;

        let daemon_divider = create_label(LabelConfig {
            frame: CGRect::new(&CGPoint::new(pad, y), &CGSize::new(field_w, 1.0)),
            text: String::new(),
            background_color: Some(ui_colors::surface_border()),
            ..Default::default()
        });
        let _: () = msg_send![daemon_divider, setAlphaValue: 0.9f64];
        add_subview(container, daemon_divider);
        y -= ui_tokens::SECTION_GAP;

        let dashboard_header = create_label(LabelConfig {
            frame: CGRect::new(&CGPoint::new(pad, y), &CGSize::new(content_w, 18.0)),
            text: "Daemon State".to_string(),
            font_size: ui_tokens::SMALL_FONT_SIZE,
            bold: true,
            text_color: primary,
            ..Default::default()
        });
        add_subview(container, dashboard_header);
        y -= 18.0 + gap;

        let snapshot = crate::qube_lifecycle::dashboard_snapshot();
        let daemon_state = &snapshot.daemon_state;

        let add_metric_row = |container: Id,
                              y: &mut f64,
                              label: &str,
                              value: &str,
                              value_color: Id,
                              width: f64,
                              pad: f64,
                              gap: f64|
         -> usize {
            let label_view = create_label(LabelConfig {
                frame: CGRect::new(&CGPoint::new(pad, *y), &CGSize::new(124.0, 18.0)),
                text: label.to_string(),
                font_size: ui_tokens::SMALL_FONT_SIZE,
                text_color: crate::ui_helpers::color_secondary_label(),
                ..Default::default()
            });
            add_subview(container, label_view);

            let value_view = create_label(LabelConfig {
                frame: CGRect::new(
                    &CGPoint::new(pad + 126.0, *y),
                    &CGSize::new((width - 126.0).max(120.0), 18.0),
                ),
                text: value.to_string(),
                font_size: ui_tokens::SMALL_FONT_SIZE,
                text_color: value_color,
                ..Default::default()
            });
            add_subview(container, value_view);
            *y -= 18.0 + gap;
            value_view as usize
        };

        let available_text = snapshot.availability_label();
        let available_color = if snapshot.available {
            ui_colors::status_granted()
        } else {
            ui_colors::status_warning()
        };
        state.quality_available_label = Some(add_metric_row(
            container,
            &mut y,
            "Availability:",
            available_text,
            available_color,
            content_w,
            pad,
            gap,
        ));
        state.quality_pending_label = Some(add_metric_row(
            container,
            &mut y,
            "Pending:",
            &daemon_state.pending_mismatches.to_string(),
            if daemon_state.pending_mismatches > 0 {
                ui_colors::status_warning()
            } else {
                secondary
            },
            content_w,
            pad,
            gap,
        ));
        state.quality_last_check_label = Some(add_metric_row(
            container,
            &mut y,
            "Last check:",
            &quality_last_check_text(&daemon_state.last_check),
            secondary,
            content_w,
            pad,
            gap,
        ));
        state.qube_report_label = Some(add_metric_row(
            container,
            &mut y,
            "Latest report:",
            &qube_report_text(daemon_state),
            secondary,
            content_w,
            pad,
            gap,
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

        container
    }
}

// ============================================================================
// Diagnostics tab
// ============================================================================

fn permission_status_text(status: PermissionStatus) -> &'static str {
    match status {
        PermissionStatus::Granted => "Granted",
        PermissionStatus::Denied => "Denied",
        PermissionStatus::NotDetermined => "Not determined",
    }
}

fn permission_status_color(status: PermissionStatus) -> Id {
    match status {
        PermissionStatus::Granted => ui_colors::status_granted(),
        PermissionStatus::Denied => ui_colors::status_denied(),
        PermissionStatus::NotDetermined => ui_colors::status_warning(),
    }
}

unsafe fn build_diagnostics_tab(
    action_handler: Id,
    frame: core_graphics::geometry::CGRect,
    config: &Config,
    state: &mut SettingsWindowState,
) -> Id {
    use core_graphics::geometry::{CGPoint, CGRect, CGSize};
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

pub(super) extern "C" fn on_mode_binding_change(
    _this: &Object,
    _cmd: objc::runtime::Sel,
    sender: Id,
) {
    unsafe {
        let tag: isize = msg_send![sender, tag];
        if mode_from_double_ctrl_tag(tag) {
            apply_mode_binding(WorkMode::Dictation, ShortcutBinding::DoubleCtrl);
            return;
        }
        if let Some(mode) = mode_from_disable_tag(tag) {
            apply_mode_binding(mode, ShortcutBinding::Disabled);
            return;
        }
        if let Some(mode) = mode_from_tag(tag) {
            start_mode_binding_recorder(mode);
        }
    }
}

pub(super) extern "C" fn on_show_hotkey_conflicts(
    _this: &Object,
    _cmd: objc::runtime::Sel,
    _sender: Id,
) {
    show_hotkey_conflicts_sheet();
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
        info!("Settings: formatting endpoint -> {}", value);
        let config = Config::load();
        let _ = config.save_to_env("LLM_FORMATTING_ENDPOINT", &value);
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
        info!("Settings: formatting model -> {}", value);
        let config = Config::load();
        let _ = config.save_to_env("LLM_FORMATTING_MODEL", &value);
    }
}

pub(super) extern "C" fn on_llm_key_changed(_this: &Object, _cmd: objc::runtime::Sel, sender: Id) {
    unsafe {
        let ns_val: Id = msg_send![sender, stringValue];
        let cstr: *const std::ffi::c_char = msg_send![ns_val, UTF8String];
        let value = std::ffi::CStr::from_ptr(cstr).to_string_lossy().to_string();
        if !value.is_empty() {
            info!("Settings: formatting API key updated (stored in Keychain)");
            let config = Config::load();
            let _ = config.save_to_env("LLM_FORMATTING_API_KEY", &value);
            update_keychain_status_labels();
        }
    }
}

pub(super) extern "C" fn on_clear_llm_key(_this: &Object, _cmd: objc::runtime::Sel, _sender: Id) {
    let field_ptr = {
        let state = SETTINGS_WINDOW_STATE
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        state.llm_key_field
    };
    clear_keychain_entry("LLM_FORMATTING_API_KEY", field_ptr);
}

pub(super) extern "C" fn on_save_api_settings(
    _this: &Object,
    _cmd: objc::runtime::Sel,
    _sender: Id,
) {
    let (llm_endpoint, llm_model, llm_key, assist_endpoint, assist_model, assist_key) = {
        let state = SETTINGS_WINDOW_STATE
            .lock()
            .unwrap_or_else(|e| e.into_inner());
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
            entries.push(("LLM_FORMATTING_ENDPOINT", value.trim().to_string()));
        }
        if let Some(ptr) = llm_model {
            let value = crate::ui_helpers::get_text_field_string(ptr as Id);
            entries.push(("LLM_FORMATTING_MODEL", value.trim().to_string()));
        }
        if let Some(ptr) = llm_key {
            let value = crate::ui_helpers::get_text_field_string(ptr as Id);
            if !value.trim().is_empty() {
                entries.push(("LLM_FORMATTING_API_KEY", value.trim().to_string()));
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

pub(super) extern "C" fn on_prompt_type_changed(
    this: &Object,
    cmd: objc::runtime::Sel,
    sender: Id,
) {
    refresh_prompt_editor_labels();
    on_prompt_load(this, cmd, sender);
}

pub(super) extern "C" fn on_prompt_load(_this: &Object, _cmd: objc::runtime::Sel, _sender: Id) {
    let prompt_type = selected_prompt_type();
    match load_prompt_content(prompt_type) {
        Ok(content) => {
            set_prompt_editor_content(&content);
            set_prompt_editor_status(
                &format!("{} prompt loaded.", prompt_display_name(prompt_type)),
                false,
            );
        }
        Err(err) => {
            set_prompt_editor_status(&format!("Failed to load prompt: {err}"), true);
        }
    }
    refresh_prompt_editor_labels();
}

pub(super) extern "C" fn on_prompt_save(_this: &Object, _cmd: objc::runtime::Sel, _sender: Id) {
    let prompt_type = selected_prompt_type();
    let content = read_prompt_editor_content();
    if content.trim().is_empty() {
        set_prompt_editor_status("Prompt is empty. Add content before saving.", true);
        return;
    }

    match save_prompt_content(prompt_type, &content) {
        Ok(()) => {
            set_prompt_editor_status(
                &format!("{} prompt saved.", prompt_display_name(prompt_type)),
                false,
            );
        }
        Err(err) => {
            set_prompt_editor_status(&format!("Failed to save prompt: {err}"), true);
        }
    }
    refresh_prompt_editor_labels();
}

pub(super) extern "C" fn on_prompt_reset(_this: &Object, _cmd: objc::runtime::Sel, _sender: Id) {
    let prompt_type = selected_prompt_type();
    match reset_prompt_content(prompt_type) {
        Ok(()) => match load_prompt_content(prompt_type) {
            Ok(content) => {
                set_prompt_editor_content(&content);
                set_prompt_editor_status(
                    &format!(
                        "{} prompt reset to default.",
                        prompt_display_name(prompt_type)
                    ),
                    false,
                );
            }
            Err(err) => {
                set_prompt_editor_status(&format!("Prompt reset but reload failed: {err}"), true);
            }
        },
        Err(err) => {
            set_prompt_editor_status(&format!("Failed to reset prompt: {err}"), true);
        }
    }
    refresh_prompt_editor_labels();
}

pub(super) extern "C" fn on_quality_refresh(_this: &Object, _cmd: objc::runtime::Sel, _sender: Id) {
    refresh_quality_dashboard();
}

pub(super) extern "C" fn on_open_qube_report(
    _this: &Object,
    _cmd: objc::runtime::Sel,
    _sender: Id,
) {
    if !crate::qube_daemon::open_latest_report() {
        warn!("Settings: no quality report available");
    }
    refresh_quality_dashboard();
}

pub(super) extern "C" fn on_diagnostics_refresh(
    _this: &Object,
    _cmd: objc::runtime::Sel,
    _sender: Id,
) {
    refresh_permission_indicators();
    refresh_diagnostics_dashboard();
}

pub(super) extern "C" fn on_copy_diagnostics(
    _this: &Object,
    _cmd: objc::runtime::Sel,
    _sender: Id,
) {
    let report = crate::os::permissions::diagnostics_report();
    let (status_ptr, secondary) = {
        let state = SETTINGS_WINDOW_STATE
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        (
            state.diagnostics_status_label,
            crate::ui_helpers::color_secondary_label(),
        )
    };
    match crate::os::clipboard::set_clipboard(&report) {
        Ok(()) => {
            if let Some(ptr) = status_ptr {
                unsafe {
                    let label = ptr as Id;
                    set_text_field_string(label, "Diagnostics copied to clipboard.");
                    let _: () = msg_send![label, setTextColor: secondary];
                }
            }
        }
        Err(err) => {
            if let Some(ptr) = status_ptr {
                unsafe {
                    let label = ptr as Id;
                    set_text_field_string(label, &format!("Failed to copy diagnostics: {err}"));
                    let _: () = msg_send![label, setTextColor: ui_colors::bubble_error_text()];
                }
            }
        }
    }
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
            let state = SETTINGS_WINDOW_STATE
                .lock()
                .unwrap_or_else(|e| e.into_inner());
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
            let state = SETTINGS_WINDOW_STATE
                .lock()
                .unwrap_or_else(|e| e.into_inner());
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
        sync_runtime_config_via_ipc();
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

pub(super) extern "C" fn on_transcription_overlay_toggled(
    _this: &Object,
    _cmd: objc::runtime::Sel,
    sender: Id,
) {
    unsafe {
        let state: isize = msg_send![sender, state];
        let enabled = state == 1;
        info!("Settings: transcription overlay -> {}", enabled);
        let config = Config::load();
        let _ = config.save_to_env(
            "TRANSCRIPTION_OVERLAY_ENABLED",
            if enabled { "1" } else { "0" },
        );
        sync_runtime_config_via_ipc();
        if !enabled {
            crate::ui::overlay::hide_transcription_overlay();
        }
        refresh_transcription_preview_panel();
    }
}

pub(super) extern "C" fn on_preview_buffer_delay_changed(
    _this: &Object,
    _cmd: objc::runtime::Sel,
    sender: Id,
) {
    unsafe {
        let value: f64 = msg_send![sender, doubleValue];
        let ms = value.round() as u64;
        info!("Settings: preview buffer delay -> {}ms", ms);
        let config = Config::load();
        let _ = config.save_to_env("CODESCRIBE_BUFFER_DELAY_MS", &ms.to_string());
        sync_runtime_config_via_ipc();
        refresh_transcription_preview_panel();
    }
}

pub(super) extern "C" fn on_preview_typing_cps_changed(
    _this: &Object,
    _cmd: objc::runtime::Sel,
    sender: Id,
) {
    unsafe {
        let value: f64 = msg_send![sender, doubleValue];
        let cps = value.max(5.0) as f32;
        info!("Settings: preview typing cps -> {:.1}", cps);
        let config = Config::load();
        let _ = config.save_to_env("CODESCRIBE_TYPING_CPS", &format!("{cps:.1}"));
        sync_runtime_config_via_ipc();
        refresh_transcription_preview_panel();
    }
}

pub(super) extern "C" fn on_preview_emit_words_max_changed(
    _this: &Object,
    _cmd: objc::runtime::Sel,
    sender: Id,
) {
    unsafe {
        let value: f64 = msg_send![sender, doubleValue];
        let words = value.round().clamp(1.0, 10.0) as u64;
        info!("Settings: preview emit words max -> {}", words);
        let config = Config::load();
        let _ = config.save_to_env("CODESCRIBE_EMIT_WORDS_MAX", &words.to_string());
        sync_runtime_config_via_ipc();
        refresh_transcription_preview_panel();
    }
}

pub(super) extern "C" fn on_preview_interim_cadence_changed(
    _this: &Object,
    _cmd: objc::runtime::Sel,
    sender: Id,
) {
    unsafe {
        let value: f64 = msg_send![sender, doubleValue];
        let secs = value.clamp(1.0, 12.0) as f32;
        info!("Settings: preview interim cadence -> {:.1}s", secs);
        let config = Config::load();
        let _ = config.save_to_env("CODESCRIBE_BUFFERED_INTERIM_SEC", &format!("{secs:.1}"));
        sync_runtime_config_via_ipc();
        refresh_transcription_preview_panel();
    }
}

pub(super) extern "C" fn on_stt_provider_changed(
    _this: &Object,
    _cmd: objc::runtime::Sel,
    sender: Id,
) {
    unsafe {
        let selected_idx: isize = msg_send![sender, indexOfSelectedItem];
        let use_local_stt = selected_idx == 0;
        info!(
            "Settings: final transcript path -> {}",
            if use_local_stt { "local" } else { "cloud" }
        );
        let config = Config::load();
        let _ = config.save_to_env("USE_LOCAL_STT", if use_local_stt { "1" } else { "0" });
        sync_runtime_config_via_ipc();
    }
}

pub(super) extern "C" fn on_stt_endpoint_changed(
    _this: &Object,
    _cmd: objc::runtime::Sel,
    sender: Id,
) {
    unsafe {
        let ns_val: Id = msg_send![sender, stringValue];
        let cstr: *const std::ffi::c_char = msg_send![ns_val, UTF8String];
        let value = std::ffi::CStr::from_ptr(cstr)
            .to_string_lossy()
            .trim()
            .to_string();
        info!("Settings: STT endpoint -> {}", value);
        let config = Config::load();
        let _ = config.save_to_env("STT_ENDPOINT", &value);
        sync_runtime_config_via_ipc();
    }
}

pub(super) extern "C" fn on_stt_key_changed(_this: &Object, _cmd: objc::runtime::Sel, sender: Id) {
    unsafe {
        let ns_val: Id = msg_send![sender, stringValue];
        let cstr: *const std::ffi::c_char = msg_send![ns_val, UTF8String];
        let value = std::ffi::CStr::from_ptr(cstr)
            .to_string_lossy()
            .trim()
            .to_string();
        if value.is_empty() {
            info!("Settings: clearing cloud STT API key from Keychain");
            if let Err(e) = keychain::delete_key("STT_API_KEY") {
                warn!("Failed to delete STT_API_KEY from Keychain: {e}");
            }
            std::env::remove_var("STT_API_KEY");
        } else {
            info!("Settings: cloud STT API key updated (stored in Keychain)");
            let config = Config::load();
            let _ = config.save_to_env("STT_API_KEY", &value);
        }
        sync_runtime_config_via_ipc();
    }
}

pub(super) extern "C" fn on_volume_changed(_this: &Object, _cmd: objc::runtime::Sel, sender: Id) {
    unsafe {
        let value: f64 = msg_send![sender, doubleValue];
        info!("Settings: sound volume -> {:.2}", value);
        let config = Config::load();
        let _ = config.save_to_env("SOUND_VOLUME", &format!("{:.2}", value));
        sync_runtime_config_via_ipc();
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
        let state = SETTINGS_WINDOW_STATE
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        state.assistive_key_field
    };
    clear_keychain_entry("LLM_ASSISTIVE_API_KEY", field_ptr);
}

pub(super) extern "C" fn on_qube_daemon_toggled(
    _this: &Object,
    _cmd: objc::runtime::Sel,
    sender: Id,
) {
    unsafe {
        let state: isize = msg_send![sender, state];
        let enabled = state == 1;
        info!("Settings: quality daemon autostart -> {}", enabled);
        let config = Config::load();
        let _ = config.save_to_env("QUBE_DAEMON_AUTOSTART", if enabled { "1" } else { "0" });
        if enabled {
            let _ = crate::qube_lifecycle::start_managed();
        } else {
            let _ = crate::qube_lifecycle::stop_managed();
        }
        refresh_quality_dashboard();
        crate::ui::tray::update_quality_label();
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
        refresh_quality_dashboard();
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
