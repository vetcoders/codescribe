//! Simple transcription overlay for non-assistive modes.
//!
//! This module provides a minimal floating overlay window that:
//! - Shows status during recording (Recording..., Processing...)
//! - Displays live streaming transcription text
//! - Supports explicit decision actions (Save/Copy/Augment) after recording
//! - Auto-hides after recording completion
//!
//! Use this for: Ctrl hold (raw), Left ⌥⌥ toggle (normal)
//! For agent chat conversations, use voice_chat_ui overlay.
//!
//! Design: macOS Tahoe Liquid Glass (NSVisualEffectView, HudWindow material)

// Allow unexpected cfgs from objc crate's msg_send! macro
// Allow unused API methods - they're part of the public interface for future use

use crate::os::clipboard;
use codescribe_core::config::{Config, OverlayPositionMode};
use core_graphics::geometry::{CGPoint, CGRect, CGSize};
use dispatch::Queue;
use objc::runtime::{Class, Object};
use objc::{msg_send, sel, sel_impl};
use objc2_app_kit::{
    NSBackingStoreType, NSVisualEffectBlendingMode, NSVisualEffectMaterial, NSVisualEffectState,
    NSWindowCollectionBehavior, NSWindowStyleMask,
};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};
use tracing::{debug, info, warn};

use crate::ui::shared::status::{UiStatus, status_from_detail};
use crate::ui_helpers::{
    add_subview, animate_fade, button_set_action, button_style, clamp_overlay_position, color_rgba,
    create_button, create_glass_effect_view_with, create_label, create_scrollable_text_view,
    ns_string, set_hidden, set_text, set_text_view_string, set_tooltip, ui_colors, ui_tokens,
    window_close, window_set_alpha, window_show,
};
use objc::declare::ClassDecl;
use objc::runtime::Sel;
use std::sync::Once;

// Type alias for Objective-C object pointers
type Id = *mut Object;

// Window level constants
const NS_FLOATING_WINDOW_LEVEL: i64 = 3;
const NS_PROGRESS_INDICATOR_STYLE_SPINNING: i64 = 1;
const NSTRACKING_MOUSE_ENTERED_AND_EXITED: u64 = 1 << 0;
const NSTRACKING_ACTIVE_ALWAYS: u64 = 1 << 7;
const NSTRACKING_IN_VISIBLE_RECT: u64 = 1 << 9;

const OVERLAY_WINDOW_WIDTH: f64 = 420.0;
const OVERLAY_WINDOW_MIN_HEIGHT: f64 = 180.0;
const OVERLAY_WINDOW_MAX_HEIGHT_RATIO: f64 = 0.5;
const OVERLAY_PADDING: f64 = 16.0;
const OVERLAY_HEADER_HEIGHT: f64 = 20.0;
const OVERLAY_STATUS_HEIGHT: f64 = 20.0;
const OVERLAY_INFO_HEIGHT: f64 = 12.0;
const OVERLAY_STATUS_WIDTH: f64 = 100.0;
const OVERLAY_HEADER_GAP: f64 = 4.0;
const OVERLAY_CONTENT_GAP: f64 = 8.0;
const OVERLAY_TEXT_MIN_HEIGHT: f64 = 44.0;
const OVERLAY_BUTTON_HEIGHT: f64 = 28.0;
const OVERLAY_BUTTON_MARGIN: f64 = 8.0;
const OVERLAY_HEADER_LABEL: &str = "CodeScribe - Dictation Overlay";

// Auto-hide delay after recording completes
const AUTO_HIDE_DELAY_SECS: u64 = 5;
const OVERLAY_LAYOUT_THROTTLE_MS: u64 = 80;
const OVERLAY_LAYOUT_HYSTERESIS_PX: f64 = 1.0;

/// Configuration for the transcription overlay
#[derive(Debug, Clone)]
pub struct TranscriptionOverlayConfig {
    /// Width of the overlay window in pixels
    pub width: f64,
    /// Height of the overlay window in pixels
    pub height: f64,
}

impl Default for TranscriptionOverlayConfig {
    fn default() -> Self {
        Self {
            width: 420.0,
            height: 180.0,
        }
    }
}

/// Source-of-truth mode for transcription overlay actions in decision mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TranscriptionActionContractMode {
    /// Copy/Augment use raw transcript captured from STT.
    Raw,
    /// Copy/Augment use last-pass/formatted transcript.
    AiFormat,
}

/// Transcription overlay state
struct TranscriptionOverlayState {
    window: Option<usize>,
    header_label: Option<usize>,
    text_scroll_view: Option<usize>,
    text_view: Option<usize>,
    status_field: Option<usize>,
    auto_hide_label: Option<usize>,
    blur_view: Option<usize>,
    copy_button: Option<usize>,
    augment_button: Option<usize>,
    save_button: Option<usize>,
    commit_button: Option<usize>,
    progress_indicator: Option<usize>,
    tracking_area: Option<usize>,
    decision_mode: bool,
    hover_active: bool,
    action_handler: Option<usize>,
    action_contract_mode: TranscriptionActionContractMode,
    raw_text: String,
    last_pass_text: String,
    accumulated_text: String,
    window_width: f64,
    min_height: f64,
    max_height: f64,
    last_applied_height: f64,
    last_layout_resize_at: Instant,
    pending_layout_resize: bool,
}

lazy_static::lazy_static! {
    static ref OVERLAY_STATE: Mutex<TranscriptionOverlayState> = Mutex::new(TranscriptionOverlayState {
        window: None,
        header_label: None,
        text_scroll_view: None,
        text_view: None,
        status_field: None,
        auto_hide_label: None,
        blur_view: None,
        copy_button: None,
        augment_button: None,
        save_button: None,
        commit_button: None,
        progress_indicator: None,
        tracking_area: None,
        decision_mode: false,
        hover_active: false,
        action_handler: None,
        action_contract_mode: TranscriptionActionContractMode::Raw,
        raw_text: String::new(),
        last_pass_text: String::new(),
        accumulated_text: String::new(),
        window_width: OVERLAY_WINDOW_WIDTH,
        min_height: OVERLAY_WINDOW_MIN_HEIGHT,
        max_height: OVERLAY_WINDOW_MIN_HEIGHT,
        last_applied_height: OVERLAY_WINDOW_MIN_HEIGHT,
        last_layout_resize_at: Instant::now(),
        pending_layout_resize: false,
    });
}

#[repr(C)]
#[derive(Clone, Copy)]
struct NSRange {
    location: usize,
    length: usize,
}

/// Flag to track if auto-hide timer is pending
static AUTO_HIDE_PENDING: AtomicBool = AtomicBool::new(false);
/// Counter to invalidate old timers
static AUTO_HIDE_GENERATION: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

// ═══════════════════════════════════════════════════════════
// Action Handler Class (for button callbacks)
// ═══════════════════════════════════════════════════════════

static ACTION_HANDLER_INIT: Once = Once::new();
static mut ACTION_HANDLER_CLASS: *const Class = std::ptr::null();

fn action_handler_class() -> *const Class {
    ACTION_HANDLER_INIT.call_once(|| unsafe {
        let superclass = Class::get("NSObject").unwrap();
        let mut decl = ClassDecl::new("TranscriptionOverlayActionHandler", superclass).unwrap();

        decl.add_method(
            sel!(onCopyTranscript:),
            on_copy_transcript as extern "C" fn(&Object, Sel, Id),
        );
        decl.add_method(
            sel!(onAugmentTranscript:),
            on_augment_transcript as extern "C" fn(&Object, Sel, Id),
        );
        decl.add_method(
            sel!(onSaveTranscript:),
            on_save_transcript as extern "C" fn(&Object, Sel, Id),
        );
        decl.add_method(
            sel!(onCommitRecording:),
            on_commit_recording as extern "C" fn(&Object, Sel, Id),
        );
        decl.add_method(
            sel!(mouseEntered:),
            on_mouse_entered as extern "C" fn(&Object, Sel, Id),
        );
        decl.add_method(
            sel!(mouseExited:),
            on_mouse_exited as extern "C" fn(&Object, Sel, Id),
        );

        ACTION_HANDLER_CLASS = decl.register();
    });
    unsafe { ACTION_HANDLER_CLASS }
}

fn action_text_for_contract(state: &TranscriptionOverlayState) -> String {
    match state.action_contract_mode {
        TranscriptionActionContractMode::Raw => {
            if !state.raw_text.trim().is_empty() {
                state.raw_text.clone()
            } else if !state.accumulated_text.trim().is_empty() {
                state.accumulated_text.clone()
            } else {
                state.last_pass_text.clone()
            }
        }
        TranscriptionActionContractMode::AiFormat => {
            if !state.last_pass_text.trim().is_empty() {
                state.last_pass_text.clone()
            } else if !state.accumulated_text.trim().is_empty() {
                state.accumulated_text.clone()
            } else {
                state.raw_text.clone()
            }
        }
    }
}

/// Handler: Copy transcript using contract source of truth.
extern "C" fn on_copy_transcript(_this: &Object, _cmd: Sel, _sender: Id) {
    let text = {
        let state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
        action_text_for_contract(&state)
    };
    if text.is_empty() {
        return;
    }
    if let Err(e) = clipboard::set_clipboard(&text) {
        warn!("Failed to copy transcript: {}", e);
        let state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
        set_status_message(&state, "Copy failed", true);
        return;
    }

    info!("Copied transcript ({} chars)", text.len());
    hide_transcription_overlay();
}

/// Handler: Augment transcript via explicit chat handoff.
extern "C" fn on_augment_transcript(_this: &Object, _cmd: Sel, _sender: Id) {
    let text = {
        let state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
        action_text_for_contract(&state)
    };
    if text.is_empty() {
        return;
    }
    crate::show_voice_chat_overlay();
    crate::show_agent_tab();
    crate::voice_chat_ui::handoff_transcript_to_chat(&text);
    hide_transcription_overlay();
}

/// Handler: Save (save already happened in controller; just close overlay)
extern "C" fn on_save_transcript(_this: &Object, _cmd: Sel, _sender: Id) {
    hide_transcription_overlay();
}

/// Handler: Commit recording (stop stream + enter decision mode)
extern "C" fn on_commit_recording(_this: &Object, _cmd: Sel, _sender: Id) {
    crate::controller::request_recording_commit();
}

extern "C" fn on_mouse_entered(_this: &Object, _cmd: Sel, _sender: Id) {
    let cancel_auto_hide = {
        let mut state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
        state.hover_active = true;
        if state.decision_mode {
            set_action_buttons_visible(&state, true);
            true
        } else {
            false
        }
    };
    if cancel_auto_hide {
        AUTO_HIDE_GENERATION.fetch_add(1, Ordering::SeqCst);
        AUTO_HIDE_PENDING.store(false, Ordering::SeqCst);
    }
}

extern "C" fn on_mouse_exited(_this: &Object, _cmd: Sel, _sender: Id) {
    let should_reschedule_auto_hide = {
        let mut state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
        state.hover_active = false;
        if state.decision_mode {
            set_action_buttons_visible(&state, true);
            true
        } else {
            set_action_buttons_visible(&state, false);
            false
        }
    };
    if should_reschedule_auto_hide {
        schedule_auto_hide();
    }
}

fn set_action_buttons_visible(state: &TranscriptionOverlayState, visible: bool) {
    if let Some(copy_ptr) = state.copy_button {
        unsafe {
            set_hidden(copy_ptr as Id, !visible);
        }
    }
    if let Some(augment_ptr) = state.augment_button {
        unsafe {
            set_hidden(augment_ptr as Id, !visible);
        }
    }
    if let Some(save_ptr) = state.save_button {
        unsafe {
            set_hidden(save_ptr as Id, !visible);
        }
    }
}

fn set_recording_button_visible(state: &TranscriptionOverlayState, visible: bool) {
    if let Some(commit_ptr) = state.commit_button {
        unsafe {
            set_hidden(commit_ptr as Id, !visible);
        }
    }
}

fn action_contract_source_label(mode: TranscriptionActionContractMode) -> &'static str {
    match mode {
        TranscriptionActionContractMode::Raw => "RAW",
        TranscriptionActionContractMode::AiFormat => "AI-FORMAT",
    }
}

fn copy_action_tooltip(mode: TranscriptionActionContractMode) -> &'static str {
    match mode {
        TranscriptionActionContractMode::Raw => "Copy RAW transcript",
        TranscriptionActionContractMode::AiFormat => "Copy last-pass/formatted transcript",
    }
}

fn augment_action_tooltip(mode: TranscriptionActionContractMode) -> &'static str {
    match mode {
        TranscriptionActionContractMode::Raw => "Open Agent overlay and hand off RAW transcript",
        TranscriptionActionContractMode::AiFormat => {
            "Open Agent overlay and hand off last-pass/formatted transcript"
        }
    }
}

fn decision_hint_text(mode: TranscriptionActionContractMode, include_auto_hide: bool) -> String {
    let base = format!(
        "Dictation overlay | Source: {} | Save closes | Augment -> Agent",
        action_contract_source_label(mode)
    );
    if include_auto_hide {
        format!("{base} | Auto-hide {}s", AUTO_HIDE_DELAY_SECS)
    } else {
        base
    }
}

fn refresh_action_contract_ui(state: &TranscriptionOverlayState, include_auto_hide_hint: bool) {
    if let Some(copy_ptr) = state.copy_button {
        unsafe {
            set_tooltip(
                copy_ptr as Id,
                copy_action_tooltip(state.action_contract_mode),
            );
        }
    }
    if let Some(augment_ptr) = state.augment_button {
        unsafe {
            set_tooltip(
                augment_ptr as Id,
                augment_action_tooltip(state.action_contract_mode),
            );
        }
    }
    if let Some(save_ptr) = state.save_button {
        unsafe {
            set_tooltip(
                save_ptr as Id,
                "Close dictation overlay (transcript already saved)",
            );
        }
    }
    if let Some(label_ptr) = state.auto_hide_label {
        unsafe {
            if include_auto_hide_hint {
                let hint = decision_hint_text(state.action_contract_mode, true);
                set_text(label_ptr as Id, &hint);
                set_tooltip(label_ptr as Id, "Transcription overlay action contract");
                set_hidden(label_ptr as Id, false);
            } else {
                set_hidden(label_ptr as Id, true);
            }
        }
    }
}

fn set_auto_hide_hint_visible(state: &TranscriptionOverlayState, visible: bool) {
    refresh_action_contract_ui(state, visible);
}

fn overlay_status_label(kind: UiStatus) -> &'static str {
    match kind {
        UiStatus::Idle => "Idle",
        UiStatus::Listening => "Listening",
        UiStatus::Processing => "Thinking",
        UiStatus::Error => "Error",
    }
}

fn overlay_top_reserved_height() -> f64 {
    OVERLAY_PADDING
        + OVERLAY_HEADER_HEIGHT
        + OVERLAY_HEADER_GAP
        + OVERLAY_INFO_HEIGHT
        + OVERLAY_CONTENT_GAP
}

fn overlay_bottom_reserved_height() -> f64 {
    OVERLAY_PADDING + OVERLAY_BUTTON_HEIGHT + OVERLAY_BUTTON_MARGIN
}

#[derive(Debug, Clone, Copy)]
struct OverlayLayoutMetrics {
    target_height: f64,
    text_viewport_height: f64,
    text_document_height: f64,
    needs_scroll: bool,
}

fn compute_overlay_layout_metrics(
    text_content_height: f64,
    min_height: f64,
    max_height: f64,
) -> OverlayLayoutMetrics {
    let clamped_content_height = text_content_height.max(OVERLAY_TEXT_MIN_HEIGHT);
    let chrome_height = overlay_top_reserved_height() + overlay_bottom_reserved_height();
    let required_window_height = clamped_content_height + chrome_height;
    let target_height = required_window_height.max(min_height).min(max_height);
    let text_viewport_height = (target_height - chrome_height).max(OVERLAY_TEXT_MIN_HEIGHT);
    let text_document_height = clamped_content_height.max(text_viewport_height);
    let needs_scroll = text_document_height > text_viewport_height + 0.5;

    OverlayLayoutMetrics {
        target_height,
        text_viewport_height,
        text_document_height,
        needs_scroll,
    }
}

fn set_status_message(state: &TranscriptionOverlayState, msg: &str, allow_spinner: bool) {
    let status_kind = status_from_detail(msg);
    let status_text = overlay_status_label(status_kind);
    let palette = status_kind.palette();

    if let Some(status_ptr) = state.status_field {
        unsafe {
            set_text(status_ptr as Id, status_text);
            set_hidden(status_ptr as Id, false);
            let status_color = color_rgba(
                palette.text.0,
                palette.text.1,
                palette.text.2,
                palette.text.3,
            );
            let _: () = msg_send![status_ptr as Id, setTextColor: status_color];

            let detail = if msg.trim().is_empty() {
                "Status: Idle".to_string()
            } else {
                format!("Status: {}", msg.trim())
            };
            set_tooltip(status_ptr as Id, &detail);
        }
    }

    let _ = crate::tray::update_tray_status(status_kind.to_tray());

    let show_spinner = allow_spinner && status_kind == UiStatus::Processing;
    if let Some(spinner_ptr) = state.progress_indicator {
        unsafe {
            set_hidden(spinner_ptr as Id, !show_spinner);
            if show_spinner {
                let _: () =
                    msg_send![spinner_ptr as Id, startAnimation: std::ptr::null::<Object>()];
            } else {
                let _: () = msg_send![spinner_ptr as Id, stopAnimation: std::ptr::null::<Object>()];
            }
        }
    }
}

fn measure_text_view_content_height(text_view: Id, width: f64) -> f64 {
    unsafe {
        let layout: Id = msg_send![text_view, layoutManager];
        let container: Id = msg_send![text_view, textContainer];
        if layout.is_null() || container.is_null() {
            return 0.0;
        }
        let _: () = msg_send![container, setContainerSize: CGSize::new(width.max(1.0), f64::MAX)];
        let _: () = msg_send![layout, ensureLayoutForTextContainer: container];
        let used_rect: CGRect = msg_send![layout, usedRectForTextContainer: container];
        used_rect.size.height.max(0.0)
    }
}

fn scroll_text_view_to_bottom(text_view: Id) {
    unsafe {
        let text: Id = msg_send![text_view, string];
        if text.is_null() {
            return;
        }
        let len: usize = msg_send![text, length];
        if len == 0 {
            return;
        }
        let range = NSRange {
            location: len,
            length: 0,
        };
        let _: () = msg_send![text_view, scrollRangeToVisible: range];
    }
}

fn resize_overlay_to_fit_text(state: &mut TranscriptionOverlayState) {
    let (window_ptr, text_scroll_ptr, text_view_ptr) =
        match (state.window, state.text_scroll_view, state.text_view) {
            (Some(window_ptr), Some(text_scroll_ptr), Some(text_view_ptr)) => {
                (window_ptr as Id, text_scroll_ptr as Id, text_view_ptr as Id)
            }
            _ => return,
        };

    let text_width = (state.window_width - OVERLAY_PADDING * 2.0).max(120.0);
    let text_content_height = measure_text_view_content_height(text_view_ptr, text_width);
    let metrics =
        compute_overlay_layout_metrics(text_content_height, state.min_height, state.max_height);

    unsafe {
        let current_frame: CGRect = msg_send![window_ptr, frame];
        let top_y = current_frame.origin.y + current_frame.size.height;
        let should_resize = (state.last_applied_height - metrics.target_height).abs()
            > OVERLAY_LAYOUT_HYSTERESIS_PX;
        let applied_height = if should_resize {
            let new_frame = CGRect {
                origin: CGPoint {
                    x: current_frame.origin.x,
                    y: top_y - metrics.target_height,
                },
                size: CGSize {
                    width: state.window_width,
                    height: metrics.target_height,
                },
            };
            let _: () = msg_send![window_ptr, setFrame: new_frame display: true];
            state.last_applied_height = metrics.target_height;
            metrics.target_height
        } else {
            current_frame.size.height
        };
        let _: () = msg_send![window_ptr, setLevel: NS_FLOATING_WINDOW_LEVEL];

        let text_frame = CGRect {
            origin: CGPoint {
                x: OVERLAY_PADDING,
                y: overlay_bottom_reserved_height(),
            },
            size: CGSize {
                width: text_width,
                height: metrics.text_viewport_height,
            },
        };
        let _: () = msg_send![text_scroll_ptr, setFrame: text_frame];

        let document_frame = CGRect {
            origin: CGPoint { x: 0.0, y: 0.0 },
            size: CGSize {
                width: text_width,
                height: metrics.text_document_height,
            },
        };
        let _: () = msg_send![text_view_ptr, setFrame: document_frame];
        let _: () =
            msg_send![text_view_ptr, setMinSize: CGSize::new(0.0, metrics.text_viewport_height)];
        let _: () = msg_send![text_scroll_ptr, setHasVerticalScroller: metrics.needs_scroll];
        if metrics.needs_scroll {
            scroll_text_view_to_bottom(text_view_ptr);
        }

        let header_y = applied_height - OVERLAY_PADDING - OVERLAY_HEADER_HEIGHT;
        let info_y = header_y - OVERLAY_HEADER_GAP - OVERLAY_INFO_HEIGHT;
        let spinner_size = 14.0;
        let spinner_x = state.window_width - OVERLAY_PADDING - spinner_size;
        let status_gap = 6.0;
        let status_max_x = spinner_x - status_gap;
        let status_width = OVERLAY_STATUS_WIDTH.min((status_max_x - OVERLAY_PADDING).max(80.0));
        let status_x = (status_max_x - status_width).max(OVERLAY_PADDING);
        let header_width = (status_x - OVERLAY_CONTENT_GAP - OVERLAY_PADDING).max(120.0);

        if let Some(header_ptr) = state.header_label {
            let header_frame = CGRect {
                origin: CGPoint {
                    x: OVERLAY_PADDING,
                    y: header_y,
                },
                size: CGSize {
                    width: header_width,
                    height: OVERLAY_HEADER_HEIGHT,
                },
            };
            let _: () = msg_send![header_ptr as Id, setFrame: header_frame];
        }

        if let Some(status_ptr) = state.status_field {
            let status_frame = CGRect {
                origin: CGPoint {
                    x: status_x,
                    y: header_y,
                },
                size: CGSize {
                    width: status_width,
                    height: OVERLAY_STATUS_HEIGHT,
                },
            };
            let _: () = msg_send![status_ptr as Id, setFrame: status_frame];
        }

        if let Some(auto_hide_ptr) = state.auto_hide_label {
            let hint_frame = CGRect {
                origin: CGPoint {
                    x: OVERLAY_PADDING,
                    y: info_y,
                },
                size: CGSize {
                    width: state.window_width - OVERLAY_PADDING * 2.0,
                    height: OVERLAY_INFO_HEIGHT,
                },
            };
            let _: () = msg_send![auto_hide_ptr as Id, setFrame: hint_frame];
        }

        if let Some(spinner_ptr) = state.progress_indicator {
            let spinner_frame = CGRect {
                origin: CGPoint {
                    x: spinner_x,
                    y: header_y + ((OVERLAY_HEADER_HEIGHT - spinner_size) / 2.0).max(0.0),
                },
                size: CGSize {
                    width: spinner_size,
                    height: spinner_size,
                },
            };
            let _: () = msg_send![spinner_ptr as Id, setFrame: spinner_frame];
        }

        if let Some(blur_ptr) = state.blur_view {
            let blur_frame = CGRect {
                origin: CGPoint { x: 0.0, y: 0.0 },
                size: CGSize {
                    width: state.window_width,
                    height: applied_height,
                },
            };
            let _: () = msg_send![blur_ptr as Id, setFrame: blur_frame];
        }

        let button_width = 100.0;
        let button_gap = 10.0;
        let row_width = button_width * 3.0 + button_gap * 2.0;
        let row_x = (state.window_width - row_width) / 2.0;
        let save_frame = CGRect {
            origin: CGPoint {
                x: row_x,
                y: OVERLAY_PADDING,
            },
            size: CGSize {
                width: button_width,
                height: OVERLAY_BUTTON_HEIGHT,
            },
        };
        let copy_frame = CGRect {
            origin: CGPoint {
                x: row_x + button_width + button_gap,
                y: OVERLAY_PADDING,
            },
            size: CGSize {
                width: button_width,
                height: OVERLAY_BUTTON_HEIGHT,
            },
        };
        let augment_frame = CGRect {
            origin: CGPoint {
                x: row_x + (button_width + button_gap) * 2.0,
                y: OVERLAY_PADDING,
            },
            size: CGSize {
                width: button_width,
                height: OVERLAY_BUTTON_HEIGHT,
            },
        };

        if let Some(save_ptr) = state.save_button {
            let _: () = msg_send![save_ptr as Id, setFrame: save_frame];
        }
        if let Some(copy_ptr) = state.copy_button {
            let _: () = msg_send![copy_ptr as Id, setFrame: copy_frame];
        }
        if let Some(augment_ptr) = state.augment_button {
            let _: () = msg_send![augment_ptr as Id, setFrame: augment_frame];
        }
    }
}

fn update_overlay_text_only(state: &TranscriptionOverlayState) {
    if let Some(text_view_ptr) = state.text_view {
        unsafe {
            set_text_view_string(text_view_ptr as Id, &state.accumulated_text);
        }
    }
}

fn update_overlay_text_and_layout(state: &mut TranscriptionOverlayState) {
    update_overlay_text_only(state);
    resize_overlay_to_fit_text(state);
    state.last_layout_resize_at = Instant::now();
    state.pending_layout_resize = false;
}

fn maybe_resize_overlay_layout(state: &mut TranscriptionOverlayState, delta: &str) {
    let structural_delta = delta
        .chars()
        .any(|ch| ch == '\n' || ch == codescribe_core::pipeline::contracts::BACKSPACE);
    let throttle = Duration::from_millis(OVERLAY_LAYOUT_THROTTLE_MS);
    let due = state.last_layout_resize_at.elapsed() >= throttle;

    if structural_delta || due || state.pending_layout_resize {
        resize_overlay_to_fit_text(state);
        state.last_layout_resize_at = Instant::now();
        state.pending_layout_resize = false;
    } else {
        state.pending_layout_resize = true;
    }
}

fn reset_overlay_to_idle(state: &TranscriptionOverlayState) {
    set_status_message(state, "Idle", false);
}

fn set_recording_status(state: &TranscriptionOverlayState, show: bool) {
    if show {
        set_status_message(state, "Listening", false);
        return;
    }
    reset_overlay_to_idle(state);
}

/// Show the transcription overlay window
pub fn show_transcription_overlay() {
    // Cancel any pending auto-hide
    AUTO_HIDE_GENERATION.fetch_add(1, Ordering::SeqCst);
    AUTO_HIDE_PENDING.store(false, Ordering::SeqCst);

    Queue::main().exec_async(|| {
        show_transcription_overlay_impl();
    });
}

fn show_transcription_overlay_impl() {
    unsafe {
        let mut state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());

        // Reuse existing window if any
        if let Some(window_ptr) = state.window {
            let window = window_ptr as Id;
            let _: () = msg_send![window, setLevel: NS_FLOATING_WINDOW_LEVEL];
            window_show(window);
            resize_overlay_to_fit_text(&mut state);
            info!("Transcription overlay reused");
            return;
        }

        state.accumulated_text.clear();
        state.raw_text.clear();
        state.last_pass_text.clear();
        state.action_contract_mode = TranscriptionActionContractMode::Raw;

        // Get classes
        let ns_window_class = Class::get("NSWindow");
        let ns_screen_class = Class::get("NSScreen");
        let ns_color_class = Class::get("NSColor");
        let ns_progress_class = Class::get("NSProgressIndicator");
        let ns_tracking_area_class = Class::get("NSTrackingArea");

        // Defensive checks for Cocoa classes
        if ns_window_class.is_none()
            || ns_screen_class.is_none()
            || ns_color_class.is_none()
            || ns_progress_class.is_none()
            || ns_tracking_area_class.is_none()
        {
            warn!("Failed to get required Cocoa classes");
            return;
        }

        let ns_window = ns_window_class.unwrap();
        let ns_screen = ns_screen_class.unwrap();
        let ns_color = ns_color_class.unwrap();
        let ns_progress = ns_progress_class.unwrap();
        let ns_tracking_area = ns_tracking_area_class.unwrap();

        // Get screen size to position the overlay
        let main_screen: Id = msg_send![ns_screen, mainScreen];
        if main_screen.is_null() {
            warn!("No main screen available");
            return;
        }
        let visible_frame: CGRect = msg_send![main_screen, visibleFrame];

        // Load config for position logic
        let config = Config::load();

        // Modern compact dimensions for Tahoe-style overlay
        let window_width = OVERLAY_WINDOW_WIDTH;
        let window_height = OVERLAY_WINDOW_MIN_HEIGHT;
        let margin = 20.0;
        let corner_radius = ui_tokens::CORNER_RADIUS_LG;
        let max_height =
            (visible_frame.size.height * OVERLAY_WINDOW_MAX_HEIGHT_RATIO).max(window_height);

        let (raw_x, raw_y) = match config.overlay_position_mode {
            OverlayPositionMode::SnappedTopRight => {
                let right_x = visible_frame.origin.x + visible_frame.size.width;
                let top_y = visible_frame.origin.y + visible_frame.size.height;
                (
                    right_x - window_width - margin,
                    top_y - window_height - margin,
                )
            }
            OverlayPositionMode::Custom => {
                let right_x = visible_frame.origin.x + visible_frame.size.width;
                let top_y = visible_frame.origin.y + visible_frame.size.height;
                let def_x = right_x - window_width - margin;
                let def_y = top_y - window_height - margin;
                (
                    config.overlay_custom_x.unwrap_or(def_x),
                    config.overlay_custom_y.unwrap_or(def_y),
                )
            }
        };
        let (x, y) = clamp_overlay_position(
            visible_frame,
            window_width,
            window_height,
            margin,
            raw_x,
            raw_y,
        );

        let frame = CGRect {
            origin: CGPoint { x, y },
            size: CGSize {
                width: window_width,
                height: window_height,
            },
        };

        // Create borderless window for modern look
        let window: Id = msg_send![ns_window, alloc];
        if window.is_null() {
            warn!("Failed to alloc NSWindow");
            return;
        }

        // Borderless + FullSizeContentView for true vibrancy effect
        let style_mask = NSWindowStyleMask::Borderless | NSWindowStyleMask::FullSizeContentView;
        let backing = NSBackingStoreType::Buffered;
        let window: Id = msg_send![
            window,
            initWithContentRect: frame
            styleMask: style_mask
            backing: backing
            defer: false
        ];
        if window.is_null() {
            warn!("Failed to init NSWindow");
            return;
        }

        // Configure window for floating overlay
        let _: () = msg_send![window, setOpaque: false];
        let clear_color: Id = msg_send![ns_color, clearColor];
        let _: () = msg_send![window, setBackgroundColor: clear_color];
        let _: () = msg_send![window, setLevel: NS_FLOATING_WINDOW_LEVEL];
        let _: () = msg_send![window, setMovableByWindowBackground: true];
        let _: () = msg_send![window, setHasShadow: true];

        // Join all spaces (follow focus)
        // Make sure the overlay shows up even when the user is in a fullscreen Space.
        let collection_behavior = NSWindowCollectionBehavior::CanJoinAllSpaces
            | NSWindowCollectionBehavior::FullScreenAuxiliary;
        let _: () = msg_send![window, setCollectionBehavior: collection_behavior];

        // Get content view
        let content_view: Id = msg_send![window, contentView];
        if content_view.is_null() {
            warn!("Failed to get content view");
            return;
        }

        // === Tahoe Liquid Glass blur ===
        let blur_frame = CGRect::new(
            &CGPoint::new(0.0, 0.0),
            &CGSize::new(window_width, window_height),
        );
        let blur_view: Id = create_glass_effect_view_with(
            blur_frame,
            NSVisualEffectMaterial::HUDWindow,
            NSVisualEffectBlendingMode::BehindWindow,
            NSVisualEffectState::Active,
        );
        let layer: Id = msg_send![blur_view, layer];
        if !layer.is_null() {
            let _: () = msg_send![layer, setCornerRadius: corner_radius];
            let _: () = msg_send![layer, setMasksToBounds: true];
            let border = ui_colors::separator();
            let border: Id = msg_send![border, colorWithAlphaComponent: 0.28f64];
            let cg_border: Id = msg_send![border, CGColor];
            let _: () = msg_send![layer, setBorderColor: cg_border];
            let _: () = msg_send![layer, setBorderWidth: 1.0f64];
        }

        // Add blur view as background
        add_subview(content_view, blur_view);

        let _: () = msg_send![window, setTitle: ns_string(OVERLAY_HEADER_LABEL)];

        let padding = OVERLAY_PADDING;
        let button_height = OVERLAY_BUTTON_HEIGHT;
        let initial_layout = compute_overlay_layout_metrics(0.0, window_height, max_height);
        let header_y = initial_layout.target_height - OVERLAY_PADDING - OVERLAY_HEADER_HEIGHT;
        let info_y = header_y - OVERLAY_HEADER_GAP - OVERLAY_INFO_HEIGHT;
        let spinner_size = 14.0;
        let spinner_x = window_width - OVERLAY_PADDING - spinner_size;
        let status_gap = 6.0;
        let status_max_x = spinner_x - status_gap;
        let status_width = OVERLAY_STATUS_WIDTH.min((status_max_x - OVERLAY_PADDING).max(80.0));
        let status_x = (status_max_x - status_width).max(OVERLAY_PADDING);
        let header_width = (status_x - OVERLAY_CONTENT_GAP - OVERLAY_PADDING).max(120.0);

        let header_label = create_label(crate::ui_helpers::LabelConfig {
            frame: CGRect::new(
                &CGPoint::new(OVERLAY_PADDING, header_y),
                &CGSize::new(header_width, OVERLAY_HEADER_HEIGHT),
            ),
            text: OVERLAY_HEADER_LABEL.to_string(),
            font_size: ui_tokens::BODY_FONT_SIZE,
            bold: true,
            text_color: ui_colors::overlay_text(),
            background_color: None,
            selectable: false,
            editable: false,
        });
        add_subview(content_view, header_label);

        let status_field = create_label(crate::ui_helpers::LabelConfig {
            frame: CGRect::new(
                &CGPoint::new(status_x, header_y),
                &CGSize::new(status_width, OVERLAY_STATUS_HEIGHT),
            ),
            text: "Idle".to_string(),
            font_size: ui_tokens::SMALL_FONT_SIZE,
            bold: true,
            text_color: ui_colors::overlay_hint_text(),
            background_color: None,
            selectable: false,
            editable: false,
        });
        let _: () = msg_send![status_field, setAlignment: 2_isize];
        add_subview(content_view, status_field);

        let auto_hide_label = create_label(crate::ui_helpers::LabelConfig {
            frame: CGRect::new(
                &CGPoint::new(OVERLAY_PADDING, info_y),
                &CGSize::new(window_width - OVERLAY_PADDING * 2.0, OVERLAY_INFO_HEIGHT),
            ),
            text: decision_hint_text(TranscriptionActionContractMode::Raw, true),
            font_size: ui_tokens::MICRO_FONT_SIZE,
            bold: false,
            text_color: ui_colors::overlay_hint_text(),
            background_color: None,
            selectable: false,
            editable: false,
        });
        add_subview(content_view, auto_hide_label);
        set_hidden(auto_hide_label, true);

        let spinner_frame = CGRect::new(
            &CGPoint::new(
                spinner_x,
                header_y + ((OVERLAY_HEADER_HEIGHT - spinner_size) / 2.0).max(0.0),
            ),
            &CGSize::new(spinner_size, spinner_size),
        );
        let spinner: Id = msg_send![ns_progress, alloc];
        let spinner: Id = msg_send![spinner, initWithFrame: spinner_frame];
        let _: () = msg_send![spinner, setStyle: NS_PROGRESS_INDICATOR_STYLE_SPINNING];
        let _: () = msg_send![spinner, setIndeterminate: true];
        let _: () = msg_send![spinner, setDisplayedWhenStopped: false];
        add_subview(content_view, spinner);
        set_hidden(spinner, true);

        // === Scrollable text view for transcription (main area) ===
        let text_frame = CGRect::new(
            &CGPoint::new(OVERLAY_PADDING, overlay_bottom_reserved_height()),
            &CGSize::new(
                (window_width - OVERLAY_PADDING * 2.0).max(120.0),
                initial_layout.text_viewport_height,
            ),
        );
        let (text_scroll_view, text_view) = create_scrollable_text_view(text_frame, false);
        let ns_font_class = Class::get("NSFont").unwrap();
        let system_font: Id = msg_send![ns_font_class, systemFontOfSize: 14.0f64];
        let _: () = msg_send![text_view, setFont: system_font];
        let text_color = ui_colors::overlay_text();
        let _: () = msg_send![text_view, setTextColor: text_color];
        let _: () = msg_send![text_view, setRichText: false];
        let _: () =
            msg_send![text_view, setMinSize: CGSize::new(0.0, initial_layout.text_viewport_height)];
        let _: () = msg_send![
            text_view,
            setMaxSize: CGSize::new((window_width - OVERLAY_PADDING * 2.0).max(120.0), f64::MAX)
        ];
        let container: Id = msg_send![text_view, textContainer];
        if !container.is_null() {
            let _: () = msg_send![container, setLineFragmentPadding: 0.0f64];
        }
        set_text_view_string(text_view, "");
        add_subview(content_view, text_scroll_view);

        // Create action handler instance
        let handler_class = action_handler_class();
        let action_handler: Id = msg_send![handler_class, alloc];
        let action_handler: Id = msg_send![action_handler, init];

        // Track hover on the overlay (show actions only on hover in decision mode)
        let tracking_opts = NSTRACKING_MOUSE_ENTERED_AND_EXITED
            | NSTRACKING_ACTIVE_ALWAYS
            | NSTRACKING_IN_VISIBLE_RECT;
        let tracking_area: Id = msg_send![ns_tracking_area, alloc];
        let tracking_area: Id = msg_send![
            tracking_area,
            initWithRect: blur_frame
            options: tracking_opts
            owner: action_handler
            userInfo: std::ptr::null::<Object>()
        ];
        let _: () = msg_send![content_view, addTrackingArea: tracking_area];

        // === Decision buttons (hidden during recording; show on hover) ===
        let button_width = 100.0;
        let button_gap = 10.0;
        let row_width = button_width * 3.0 + button_gap * 2.0;
        let row_x = (window_width - row_width) / 2.0;
        let commit_x = (window_width - button_width) / 2.0;

        let save_frame = CGRect {
            origin: CGPoint {
                x: row_x,
                y: padding,
            },
            size: CGSize {
                width: button_width,
                height: button_height,
            },
        };
        let copy_frame = CGRect {
            origin: CGPoint {
                x: row_x + button_width + button_gap,
                y: padding,
            },
            size: CGSize {
                width: button_width,
                height: button_height,
            },
        };
        let augment_frame = CGRect {
            origin: CGPoint {
                x: row_x + (button_width + button_gap) * 2.0,
                y: padding,
            },
            size: CGSize {
                width: button_width,
                height: button_height,
            },
        };
        let commit_frame = CGRect {
            origin: CGPoint {
                x: commit_x,
                y: padding,
            },
            size: CGSize {
                width: button_width,
                height: button_height,
            },
        };

        let save_button = create_button(save_frame, "Save", button_style::ROUNDED);
        let copy_button = create_button(copy_frame, "Copy", button_style::ROUNDED);
        let augment_button = create_button(augment_frame, "Augment", button_style::ROUNDED);
        let commit_button = create_button(commit_frame, "Finish", button_style::ROUNDED);
        set_tooltip(
            copy_button,
            copy_action_tooltip(TranscriptionActionContractMode::Raw),
        );
        set_tooltip(
            augment_button,
            augment_action_tooltip(TranscriptionActionContractMode::Raw),
        );
        set_tooltip(
            save_button,
            "Close dictation overlay (transcript already saved)",
        );
        set_tooltip(commit_button, "Stop recording and enter decision mode");

        button_set_action(save_button, action_handler, sel!(onSaveTranscript:));
        button_set_action(copy_button, action_handler, sel!(onCopyTranscript:));
        button_set_action(augment_button, action_handler, sel!(onAugmentTranscript:));
        button_set_action(commit_button, action_handler, sel!(onCommitRecording:));

        add_subview(content_view, save_button);
        add_subview(content_view, copy_button);
        add_subview(content_view, augment_button);
        add_subview(content_view, commit_button);

        set_hidden(save_button, true);
        set_hidden(copy_button, true);
        set_hidden(augment_button, true);
        set_hidden(commit_button, true);

        // Show the window with fade-in animation
        window_set_alpha(window, 0.0);
        window_show(window);
        animate_fade(window, 1.0, 0.2);

        state.window = Some(window as usize);
        state.header_label = Some(header_label as usize);
        state.text_scroll_view = Some(text_scroll_view as usize);
        state.text_view = Some(text_view as usize);
        state.status_field = Some(status_field as usize);
        state.auto_hide_label = Some(auto_hide_label as usize);
        state.blur_view = Some(blur_view as usize);
        state.copy_button = Some(copy_button as usize);
        state.augment_button = Some(augment_button as usize);
        state.save_button = Some(save_button as usize);
        state.commit_button = Some(commit_button as usize);
        state.progress_indicator = Some(spinner as usize);
        state.tracking_area = Some(tracking_area as usize);
        state.decision_mode = false;
        state.hover_active = false;
        state.action_handler = Some(action_handler as usize);
        state.window_width = window_width;
        state.min_height = window_height;
        state.max_height = max_height;
        state.last_applied_height = window_height;
        state.last_layout_resize_at = Instant::now();
        state.pending_layout_resize = false;

        refresh_action_contract_ui(&state, false);
        reset_overlay_to_idle(&state);
        resize_overlay_to_fit_text(&mut state);

        info!("Transcription overlay shown (Tahoe-style with HudWindow vibrancy)");
    }
}

/// Update the status text in the overlay
pub fn update_transcription_status(status: &str) {
    let status_owned = status.to_string();
    Queue::main().exec_async(move || {
        update_transcription_status_impl(&status_owned);
    });
}

fn update_transcription_status_impl(status: &str) {
    let state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
    set_status_message(&state, status, true);
}

/// Append a delta (streaming token) to the overlay text
pub fn append_transcription_delta(delta: &str) {
    let delta_owned = delta.to_string();
    Queue::main().exec_async(move || {
        append_transcription_delta_impl(&delta_owned);
    });
}

fn append_transcription_delta_impl(delta: &str) {
    let mut state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
    codescribe_core::pipeline::contracts::TranscriptDelta::from_raw(delta)
        .apply(&mut state.accumulated_text);
    update_overlay_text_only(&state);
    maybe_resize_overlay_layout(&mut state, delta);
}

/// Set the full text in the overlay
pub fn set_transcription_text(text: &str) {
    let text_owned = text.to_string();
    Queue::main().exec_async(move || {
        set_transcription_text_impl(&text_owned);
    });
}

fn set_transcription_text_impl(text: &str) {
    let mut state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
    state.accumulated_text = text.to_string();
    state.last_pass_text = text.to_string();
    update_overlay_text_and_layout(&mut state);
}

/// Set decision-mode action contract payload.
///
/// `mode` defines whether `Copy`/`Augment` use RAW or last-pass text.
pub fn set_transcription_action_contract(
    raw_text: &str,
    last_pass_text: &str,
    mode: TranscriptionActionContractMode,
) {
    let raw_text_owned = raw_text.to_string();
    let last_pass_owned = last_pass_text.to_string();
    let mode_copy = mode;
    Queue::main().exec_async(move || {
        let mut state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
        state.raw_text = raw_text_owned;
        state.last_pass_text = last_pass_owned;
        state.action_contract_mode = mode_copy;
        state.accumulated_text = action_text_for_contract(&state);
        refresh_action_contract_ui(&state, state.decision_mode);
        update_overlay_text_and_layout(&mut state);
    });
}

/// Get the accumulated text from the overlay
pub fn get_transcription_text() -> String {
    let state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
    state.accumulated_text.clone()
}

/// Clear the text content of the overlay
pub fn clear_transcription_text() {
    Queue::main().exec_async(|| {
        clear_transcription_text_impl();
    });
}

fn clear_transcription_text_impl() {
    let mut state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
    state.accumulated_text.clear();
    state.raw_text.clear();
    state.last_pass_text.clear();
    state.action_contract_mode = TranscriptionActionContractMode::Raw;

    if let Some(text_view_ptr) = state.text_view {
        unsafe {
            set_text_view_string(text_view_ptr as Id, "");
        }
    }
    resize_overlay_to_fit_text(&mut state);
    state.last_layout_resize_at = Instant::now();
    state.pending_layout_resize = false;
    if let Some(copy_ptr) = state.copy_button {
        unsafe {
            set_hidden(copy_ptr as Id, true);
        }
    }
    if let Some(augment_ptr) = state.augment_button {
        unsafe {
            set_hidden(augment_ptr as Id, true);
        }
    }
    if let Some(save_ptr) = state.save_button {
        unsafe {
            set_hidden(save_ptr as Id, true);
        }
    }
    if let Some(commit_ptr) = state.commit_button {
        unsafe {
            set_hidden(commit_ptr as Id, true);
        }
    }
    set_auto_hide_hint_visible(&state, false);
    if let Some(spinner_ptr) = state.progress_indicator {
        unsafe {
            set_hidden(spinner_ptr as Id, true);
        }
    }
    state.decision_mode = false;
    state.hover_active = false;
    reset_overlay_to_idle(&state);
}

/// Check if the transcription overlay is currently visible
pub fn is_transcription_overlay_visible() -> bool {
    let state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
    state.window.is_some()
}

/// Schedule auto-hide after delay (call this when recording finishes)
pub fn schedule_auto_hide() {
    let generation = AUTO_HIDE_GENERATION.fetch_add(1, Ordering::SeqCst) + 1;
    AUTO_HIDE_PENDING.store(true, Ordering::SeqCst);

    Queue::main().exec_async(|| {
        let state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
        set_auto_hide_hint_visible(&state, true);
    });

    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_secs(AUTO_HIDE_DELAY_SECS));

        if should_auto_hide(generation) {
            hide_transcription_overlay();
            debug!(
                "Transcription overlay auto-hidden after {}s",
                AUTO_HIDE_DELAY_SECS
            );
        } else {
            debug!("Auto-hide skipped");
        }
    });
}

fn should_auto_hide(expected_generation: u64) -> bool {
    if AUTO_HIDE_GENERATION.load(Ordering::SeqCst) != expected_generation
        || !AUTO_HIDE_PENDING.load(Ordering::SeqCst)
    {
        return false;
    }

    let hovered = {
        let state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
        state.hover_active
    };

    !hovered
}

/// Enter decision mode: show actions on hover for the current transcript
pub fn enter_decision_mode() {
    Queue::main().exec_async(|| {
        let mut state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
        state.decision_mode = true;
        set_action_buttons_visible(&state, true);
        set_auto_hide_hint_visible(&state, true);
        set_recording_button_visible(&state, false);
        // Clear recording indicator, restore white text color
        set_recording_status(&state, false);
    });
}

/// Enter recording mode: hide actions, show recording indicator
pub fn enter_recording_mode() {
    Queue::main().exec_async(|| {
        let mut state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
        state.decision_mode = false;
        state.hover_active = false;
        set_action_buttons_visible(&state, false);
        set_auto_hide_hint_visible(&state, false);
        set_recording_button_visible(&state, true);
        // Show recording indicator (red dot + text), no spinner
        set_recording_status(&state, true);
    });
}

/// Hide the transcription overlay window (with fade-out animation)
pub fn hide_transcription_overlay() {
    // Cancel any pending auto-hide
    AUTO_HIDE_PENDING.store(false, Ordering::SeqCst);

    Queue::main().exec_async(|| {
        hide_transcription_overlay_impl();
    });
}

/// Closes a window by raw pointer (used for delayed close after animation)
fn close_window_by_ptr(window_ptr: usize) {
    unsafe {
        window_close(window_ptr as Id);
    }
}

fn hide_transcription_overlay_impl() {
    let mut state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(window_ptr) = state.window.take() {
        let window = window_ptr as Id;

        // Fade out animation (0.15s)
        unsafe {
            animate_fade(window, 0.0, 0.15);
        }

        // Close window after brief delay for animation
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(200));
            Queue::main().exec_async(move || {
                close_window_by_ptr(window_ptr);
            });
        });

        debug!("Transcription overlay hidden");
    }
    state.header_label = None;
    state.text_scroll_view = None;
    state.text_view = None;
    state.status_field = None;
    state.auto_hide_label = None;
    state.blur_view = None;
    state.copy_button = None;
    state.augment_button = None;
    state.save_button = None;
    state.commit_button = None;
    state.progress_indicator = None;
    state.tracking_area = None;
    state.decision_mode = false;
    state.hover_active = false;
    state.action_handler = None;
    state.action_contract_mode = TranscriptionActionContractMode::Raw;
    state.last_applied_height = OVERLAY_WINDOW_MIN_HEIGHT;
    state.last_layout_resize_at = Instant::now();
    state.pending_layout_resize = false;
    // Note: accumulated_text is NOT cleared here - it's needed for clipboard copy
}

#[cfg(test)]
mod tests {
    use super::*;
    use serial_test::serial;

    #[test]
    fn test_transcription_text() {
        // Just verify the function doesn't panic
        let _ = get_transcription_text();
    }

    #[test]
    fn test_overlay_config_default() {
        let config = TranscriptionOverlayConfig::default();
        assert_eq!(config.width, 420.0);
        assert_eq!(config.height, 180.0);
    }

    #[test]
    fn test_is_overlay_visible_returns_bool() {
        // Just verify the function returns a bool without panic
        let visible = is_transcription_overlay_visible();
        let _ = visible;
    }

    #[test]
    #[serial]
    fn test_auto_hide_generation() {
        // Test that generation counter increments
        let gen1 = AUTO_HIDE_GENERATION.load(Ordering::SeqCst);
        AUTO_HIDE_GENERATION.fetch_add(1, Ordering::SeqCst);
        let gen2 = AUTO_HIDE_GENERATION.load(Ordering::SeqCst);
        assert_eq!(gen2, gen1 + 1);
    }

    #[test]
    fn test_auto_hide_delay_seconds() {
        assert_eq!(AUTO_HIDE_DELAY_SECS, 5);
    }

    #[test]
    #[serial]
    fn test_auto_hide_hover_guard() {
        AUTO_HIDE_GENERATION.store(42, Ordering::SeqCst);
        AUTO_HIDE_PENDING.store(true, Ordering::SeqCst);

        {
            let mut state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
            state.hover_active = true;
        }
        assert!(!should_auto_hide(42));

        {
            let mut state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
            state.hover_active = false;
        }
        assert!(should_auto_hide(42));
    }

    #[test]
    fn test_layout_metrics_scroll_transition() {
        let min_height = OVERLAY_WINDOW_MIN_HEIGHT;
        let max_height = min_height + 80.0;

        let compact = compute_overlay_layout_metrics(40.0, min_height, max_height);
        assert!(!compact.needs_scroll);
        assert!(compact.target_height >= min_height);

        let grown = compute_overlay_layout_metrics(120.0, min_height, max_height);
        assert!(!grown.needs_scroll);
        assert!(grown.target_height > compact.target_height);

        let overflow = compute_overlay_layout_metrics(420.0, min_height, max_height);
        assert!((overflow.target_height - max_height).abs() < f64::EPSILON);
        assert!(overflow.needs_scroll);
        assert!(overflow.text_document_height > overflow.text_viewport_height);
    }

    #[test]
    fn test_layout_metrics_mobile_like_compact_window() {
        let min_height = OVERLAY_WINDOW_MIN_HEIGHT;
        let max_height = min_height + 24.0;

        let compact = compute_overlay_layout_metrics(360.0, min_height, max_height);
        assert!((compact.target_height - max_height).abs() < f64::EPSILON);
        assert!(compact.needs_scroll);
        assert!(compact.text_viewport_height >= OVERLAY_TEXT_MIN_HEIGHT);
    }

    #[test]
    fn test_overlay_status_labels_are_canonical() {
        assert_eq!(
            overlay_status_label(status_from_detail("Listening...")),
            "Listening"
        );
        assert_eq!(
            overlay_status_label(status_from_detail("Thinking...")),
            "Thinking"
        );
        assert_eq!(overlay_status_label(status_from_detail("Idle")), "Idle");
        assert_eq!(
            overlay_status_label(status_from_detail("Backend failed")),
            "Error"
        );
        assert_eq!(overlay_status_label(status_from_detail("??")), "Idle");
    }

    #[test]
    #[serial]
    fn test_action_text_uses_raw_contract_source_in_raw_mode() {
        let mut state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
        state.action_contract_mode = TranscriptionActionContractMode::Raw;
        state.raw_text = "raw transcript".to_string();
        state.accumulated_text = "overlay preview".to_string();
        state.last_pass_text = "final last-pass".to_string();

        let text = action_text_for_contract(&state);
        assert_eq!(text, "raw transcript");
    }

    #[test]
    #[serial]
    fn test_action_text_uses_last_pass_contract_source_in_ai_mode() {
        let mut state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
        state.action_contract_mode = TranscriptionActionContractMode::AiFormat;
        state.raw_text = "raw transcript".to_string();
        state.accumulated_text = "overlay preview".to_string();
        state.last_pass_text = "final last-pass".to_string();

        let text = action_text_for_contract(&state);
        assert_eq!(text, "final last-pass");
    }

    #[test]
    #[serial]
    fn test_action_text_ai_mode_falls_back_to_overlay_preview() {
        let mut state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
        state.action_contract_mode = TranscriptionActionContractMode::AiFormat;
        state.raw_text = "raw transcript".to_string();
        state.accumulated_text = "overlay preview".to_string();
        state.last_pass_text.clear();

        let text = action_text_for_contract(&state);
        assert_eq!(text, "overlay preview");
    }
}
