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
use objc2_app_kit::NSWindowStyleMask;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};
use tracing::{debug, info, warn};

use crate::ui::shared::status::{UiStatus, status_from_detail};
use crate::ui_helpers::{
    add_subview, animate_fade, button_set_action, button_style, clamp_overlay_position,
    create_borderless_tafla_window, create_button, create_label, create_scrollable_text_view,
    create_tafla_single_shell, ns_string, release_object, set_hidden, set_text,
    set_text_view_string, set_tooltip, ui_colors, ui_tokens, window_discard, window_set_alpha,
    window_show,
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

// Auto-hide delay after recording completes (configurable via env)
const DEFAULT_AUTO_HIDE_DELAY_SECS: u64 = 15;
const MIN_AUTO_HIDE_DELAY_SECS: u64 = 3;
const MAX_AUTO_HIDE_DELAY_SECS: u64 = 60;
const OVERLAY_LAYOUT_THROTTLE_MS: u64 = 80;
const OVERLAY_LAYOUT_HYSTERESIS_PX: f64 = 1.0;

fn parse_auto_hide_delay_secs(raw: Option<&str>) -> u64 {
    raw.and_then(|v| v.parse::<u64>().ok())
        .map(|v| v.clamp(MIN_AUTO_HIDE_DELAY_SECS, MAX_AUTO_HIDE_DELAY_SECS))
        .unwrap_or(DEFAULT_AUTO_HIDE_DELAY_SECS)
}

fn auto_hide_delay_secs() -> u64 {
    static DELAY: OnceLock<u64> = OnceLock::new();
    *DELAY.get_or_init(|| {
        parse_auto_hide_delay_secs(
            std::env::var("TRANSCRIPTION_OVERLAY_AUTO_HIDE_SECS")
                .ok()
                .as_deref(),
        )
    })
}

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

/// Snapshot of widget pointers + layout params for AppKit calls outside lock scope.
///
/// DEADLOCK PREVENTION: extract this while holding `OVERLAY_STATE`, then
/// **drop the lock** before using the pointers in AppKit `msg_send!` calls.
/// AppKit can spin a nested run-loop during `setFrame:display:`, `orderFront:`,
/// etc., and pending `Queue::main().exec_async` blocks that also lock
/// `OVERLAY_STATE` will deadlock on the non-reentrant `std::sync::Mutex`.
#[derive(Clone)]
struct OverlaySnapshot {
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
    window_width: f64,
    min_height: f64,
    max_height: f64,
    last_applied_height: f64,
}

impl OverlaySnapshot {
    fn from_state(state: &TranscriptionOverlayState) -> Self {
        Self {
            window: state.window,
            header_label: state.header_label,
            text_scroll_view: state.text_scroll_view,
            text_view: state.text_view,
            status_field: state.status_field,
            auto_hide_label: state.auto_hide_label,
            blur_view: state.blur_view,
            copy_button: state.copy_button,
            augment_button: state.augment_button,
            save_button: state.save_button,
            commit_button: state.commit_button,
            progress_indicator: state.progress_indicator,
            window_width: state.window_width,
            min_height: state.min_height,
            max_height: state.max_height,
            last_applied_height: state.last_applied_height,
        }
    }
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
        TranscriptionActionContractMode::Raw => state.raw_text.clone(),
        TranscriptionActionContractMode::AiFormat => state.last_pass_text.clone(),
    }
}

fn display_text_for_state(state: &TranscriptionOverlayState) -> String {
    let text = if state.accumulated_text.trim().is_empty() {
        action_text_for_contract(state)
    } else {
        state.accumulated_text.clone()
    };
    overlay_visible_text(&text, state.decision_mode).to_string()
}

/// Handler: Copy transcript using contract source of truth.
extern "C" fn on_copy_transcript(_this: &Object, _cmd: Sel, _sender: Id) {
    let (text, snap) = {
        let state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
        (
            action_text_for_contract(&state),
            OverlaySnapshot::from_state(&state),
        )
    };
    if text.is_empty() {
        return;
    }
    if let Err(e) = clipboard::set_clipboard(&text) {
        warn!("Failed to copy transcript: {}", e);
        set_status_message_unlocked(&snap, "Copy failed", true);
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
    let (cancel_auto_hide, snap) = {
        let mut state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
        state.hover_active = true;
        let dm = state.decision_mode;
        (dm, OverlaySnapshot::from_state(&state))
    }; // Lock dropped before AppKit calls.
    if cancel_auto_hide {
        set_action_buttons_visible_unlocked(&snap, true);
        AUTO_HIDE_GENERATION.fetch_add(1, Ordering::SeqCst);
        AUTO_HIDE_PENDING.store(false, Ordering::SeqCst);
    }
}

extern "C" fn on_mouse_exited(_this: &Object, _cmd: Sel, _sender: Id) {
    let (decision_mode, snap) = {
        let mut state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
        state.hover_active = false;
        (state.decision_mode, OverlaySnapshot::from_state(&state))
    }; // Lock dropped before AppKit calls.
    if decision_mode {
        set_action_buttons_visible_unlocked(&snap, true);
        schedule_auto_hide();
    } else {
        set_action_buttons_visible_unlocked(&snap, false);
    }
}

/// Show/hide action buttons. Call ONLY outside the `OVERLAY_STATE` lock.
fn set_action_buttons_visible_unlocked(snap: &OverlaySnapshot, visible: bool) {
    if let Some(copy_ptr) = snap.copy_button {
        unsafe {
            set_hidden(copy_ptr as Id, !visible);
        }
    }
    if let Some(augment_ptr) = snap.augment_button {
        unsafe {
            set_hidden(augment_ptr as Id, !visible);
        }
    }
    if let Some(save_ptr) = snap.save_button {
        unsafe {
            set_hidden(save_ptr as Id, !visible);
        }
    }
}

/// Show/hide commit button. Call ONLY outside the `OVERLAY_STATE` lock.
fn set_recording_button_visible_unlocked(snap: &OverlaySnapshot, visible: bool) {
    if let Some(commit_ptr) = snap.commit_button {
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
        format!("{base} | Auto-hide {}s", auto_hide_delay_secs())
    } else {
        base
    }
}

/// Refresh action contract tooltips/hints. Call ONLY outside the `OVERLAY_STATE` lock.
fn refresh_action_contract_ui_unlocked(
    snap: &OverlaySnapshot,
    mode: TranscriptionActionContractMode,
    include_auto_hide_hint: bool,
) {
    if let Some(copy_ptr) = snap.copy_button {
        unsafe {
            set_tooltip(copy_ptr as Id, copy_action_tooltip(mode));
        }
    }
    if let Some(augment_ptr) = snap.augment_button {
        unsafe {
            set_tooltip(augment_ptr as Id, augment_action_tooltip(mode));
        }
    }
    if let Some(save_ptr) = snap.save_button {
        unsafe {
            set_tooltip(
                save_ptr as Id,
                "Close dictation overlay (transcript already saved)",
            );
        }
    }
    if let Some(label_ptr) = snap.auto_hide_label {
        unsafe {
            if include_auto_hide_hint {
                let hint = decision_hint_text(mode, true);
                set_text(label_ptr as Id, &hint);
                set_tooltip(label_ptr as Id, "Transcription overlay action contract");
                set_hidden(label_ptr as Id, false);
            } else {
                set_hidden(label_ptr as Id, true);
            }
        }
    }
}

/// Show/hide auto-hide hint. Call ONLY outside the `OVERLAY_STATE` lock.
fn set_auto_hide_hint_visible_unlocked(
    snap: &OverlaySnapshot,
    mode: TranscriptionActionContractMode,
    visible: bool,
) {
    refresh_action_contract_ui_unlocked(snap, mode, visible);
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

/// Update status label + spinner. Call ONLY outside the `OVERLAY_STATE` lock.
fn set_status_message_unlocked(snap: &OverlaySnapshot, msg: &str, allow_spinner: bool) {
    let status_kind = status_from_detail(msg);
    let status_text = overlay_status_label(status_kind);

    if let Some(status_ptr) = snap.status_field {
        unsafe {
            set_text(status_ptr as Id, status_text);
            set_hidden(status_ptr as Id, false);
            let status_color = status_kind.text_color();
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
    if let Some(spinner_ptr) = snap.progress_indicator {
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

/// Resize overlay window to fit text content. Call ONLY outside the `OVERLAY_STATE` lock.
/// Returns the new `last_applied_height` for write-back to state.
fn resize_overlay_unlocked(snap: &OverlaySnapshot) -> f64 {
    let (window_ptr, text_scroll_ptr, text_view_ptr) =
        match (snap.window, snap.text_scroll_view, snap.text_view) {
            (Some(w), Some(ts), Some(tv)) => (w as Id, ts as Id, tv as Id),
            _ => return snap.last_applied_height,
        };

    let text_width = (snap.window_width - OVERLAY_PADDING * 2.0).max(120.0);
    let text_content_height = measure_text_view_content_height(text_view_ptr, text_width);
    let metrics =
        compute_overlay_layout_metrics(text_content_height, snap.min_height, snap.max_height);

    unsafe {
        let current_frame: CGRect = msg_send![window_ptr, frame];
        let top_y = current_frame.origin.y + current_frame.size.height;
        let should_resize =
            (snap.last_applied_height - metrics.target_height).abs() > OVERLAY_LAYOUT_HYSTERESIS_PX;
        let applied_height = if should_resize {
            let new_frame = CGRect {
                origin: CGPoint {
                    x: current_frame.origin.x,
                    y: top_y - metrics.target_height,
                },
                size: CGSize {
                    width: snap.window_width,
                    height: metrics.target_height,
                },
            };
            let _: () = msg_send![window_ptr, setFrame: new_frame display: true];
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
        let spinner_x = snap.window_width - OVERLAY_PADDING - spinner_size;
        let status_gap = 6.0;
        let status_max_x = spinner_x - status_gap;
        let status_width = OVERLAY_STATUS_WIDTH.min((status_max_x - OVERLAY_PADDING).max(80.0));
        let status_x = (status_max_x - status_width).max(OVERLAY_PADDING);
        let header_width = (status_x - OVERLAY_CONTENT_GAP - OVERLAY_PADDING).max(120.0);

        if let Some(header_ptr) = snap.header_label {
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

        if let Some(status_ptr) = snap.status_field {
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

        if let Some(auto_hide_ptr) = snap.auto_hide_label {
            let hint_frame = CGRect {
                origin: CGPoint {
                    x: OVERLAY_PADDING,
                    y: info_y,
                },
                size: CGSize {
                    width: snap.window_width - OVERLAY_PADDING * 2.0,
                    height: OVERLAY_INFO_HEIGHT,
                },
            };
            let _: () = msg_send![auto_hide_ptr as Id, setFrame: hint_frame];
        }

        if let Some(spinner_ptr) = snap.progress_indicator {
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

        if let Some(blur_ptr) = snap.blur_view {
            let blur_frame = CGRect {
                origin: CGPoint { x: 0.0, y: 0.0 },
                size: CGSize {
                    width: snap.window_width,
                    height: applied_height,
                },
            };
            let _: () = msg_send![blur_ptr as Id, setFrame: blur_frame];
        }

        let button_width = 100.0;
        let button_gap = 10.0;
        let row_width = button_width * 3.0 + button_gap * 2.0;
        let row_x = (snap.window_width - row_width) / 2.0;
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

        if let Some(save_ptr) = snap.save_button {
            let _: () = msg_send![save_ptr as Id, setFrame: save_frame];
        }
        if let Some(copy_ptr) = snap.copy_button {
            let _: () = msg_send![copy_ptr as Id, setFrame: copy_frame];
        }
        if let Some(augment_ptr) = snap.augment_button {
            let _: () = msg_send![augment_ptr as Id, setFrame: augment_frame];
        }

        applied_height
    }
}

/// Update the overlay text content. Call ONLY outside the `OVERLAY_STATE` lock.
fn update_overlay_text_unlocked(text_view_ptr: Option<usize>, visible_text: &str) {
    if let Some(tv_ptr) = text_view_ptr {
        unsafe {
            set_text_view_string(tv_ptr as Id, visible_text);
        }
    }
}

fn overlay_visible_text(text: &str, decision_mode: bool) -> &str {
    if decision_mode || !overlay_live_preview_uses_stable_text() {
        // Decision mode must show exact contract payload without preview filtering.
        text
    } else {
        // Live preview shows only complete word boundaries to avoid jittery partial tails.
        stable_overlay_preview_text(text)
    }
}

fn overlay_live_preview_uses_stable_text() -> bool {
    std::env::var("CODESCRIBE_OVERLAY_STABLE_PREVIEW")
        .ok()
        .map(|v| {
            matches!(
                v.trim().to_ascii_lowercase().as_str(),
                "1" | "true" | "yes" | "on"
            )
        })
        .unwrap_or(false)
}

fn stable_overlay_preview_text(text: &str) -> &str {
    if text.is_empty() {
        return text;
    }

    let ends_stable = text
        .chars()
        .last()
        .map(is_preview_boundary_char)
        .unwrap_or(false);
    if ends_stable {
        return text;
    }

    let mut last_boundary_idx = None;
    for (idx, ch) in text.char_indices() {
        if is_preview_boundary_char(ch) {
            last_boundary_idx = Some(idx + ch.len_utf8());
        }
    }

    match last_boundary_idx {
        Some(idx) => &text[..idx],
        None => text,
    }
}

fn is_preview_boundary_char(ch: char) -> bool {
    ch.is_whitespace()
        || matches!(
            ch,
            '.' | ','
                | ';'
                | ':'
                | '!'
                | '?'
                | ')'
                | '('
                | ']'
                | '['
                | '}'
                | '{'
                | '"'
                | '\''
                | '…'
                | '—'
                | '-'
        )
}

// NOTE: update_overlay_text_and_layout and maybe_resize_overlay_layout were removed.
// Their logic is now inlined into callers using the extract-drop-execute pattern
// to prevent deadlocks. See append_transcription_delta_impl, set_transcription_text_impl, etc.

/// Reset status to idle. Call ONLY outside the `OVERLAY_STATE` lock.
fn reset_overlay_to_idle_unlocked(snap: &OverlaySnapshot) {
    set_status_message_unlocked(snap, "Idle", false);
}

/// Toggle recording status indicator. Call ONLY outside the `OVERLAY_STATE` lock.
fn set_recording_status_unlocked(snap: &OverlaySnapshot, show: bool) {
    if show {
        set_status_message_unlocked(snap, "Listening", false);
        return;
    }
    reset_overlay_to_idle_unlocked(snap);
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
            // DEADLOCK PREVENTION: extract snapshot, drop lock before AppKit calls.
            let snap = OverlaySnapshot::from_state(&state);
            drop(state);

            let window = window_ptr as Id;
            let _: () = msg_send![window, setLevel: NS_FLOATING_WINDOW_LEVEL];
            window_show(window);
            let new_h = resize_overlay_unlocked(&snap);
            {
                let mut state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
                state.last_applied_height = new_h;
                state.last_layout_resize_at = Instant::now();
                state.pending_layout_resize = false;
            }
            info!("Transcription overlay reused");
            return;
        }

        state.accumulated_text.clear();
        state.raw_text.clear();
        state.last_pass_text.clear();
        state.action_contract_mode = TranscriptionActionContractMode::Raw;
        drop(state); // Release lock BEFORE heavy AppKit widget creation.

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

        let ns_screen = ns_screen_class.unwrap();
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
        let corner_radius = ui_tokens::SURFACE_RADIUS;
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

        let style_mask = NSWindowStyleMask::Borderless | NSWindowStyleMask::FullSizeContentView;
        let Some(window) = create_borderless_tafla_window(frame, style_mask, true) else {
            warn!("Failed to init NSWindow");
            return;
        };

        // Get content view
        let window_content_view: Id = msg_send![window, contentView];
        if window_content_view.is_null() {
            warn!("Failed to get content view");
            discard_overlay_window(window, None);
            return;
        }

        // === Tahoe Liquid Glass blur ===
        let blur_frame = CGRect::new(
            &CGPoint::new(0.0, 0.0),
            &CGSize::new(window_width, window_height),
        );
        let Some((blur_view, content_view)) =
            create_tafla_single_shell(window_content_view, blur_frame)
        else {
            warn!("Failed to attach transcription overlay shell");
            discard_overlay_window(window, None);
            return;
        };
        let layer: Id = msg_send![blur_view, layer];
        if !layer.is_null() {
            let _: () = msg_send![layer, setCornerRadius: corner_radius];
            let _: () = msg_send![layer, setMasksToBounds: true];
            let border = ui_colors::overlay_sheet_border();
            let cg_border: Id = msg_send![border, CGColor];
            let _: () = msg_send![layer, setBorderColor: cg_border];
            let _: () = msg_send![layer, setBorderWidth: 1.0f64];
        }

        // Add blur view as background, then mount overlay controls via glass `contentView`.
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

        let save_button = create_button(save_frame, "Save", button_style::GLASS);
        let copy_button = create_button(copy_frame, "Copy", button_style::ROUNDED);
        let augment_button = create_button(augment_frame, "Augment", button_style::ROUNDED);
        let commit_button = create_button(commit_frame, "Finish", button_style::GLASS);
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

        // Re-acquire lock to store widget pointers (quick write, no AppKit calls).
        let mut state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
        // Guard: if another path filled window while we were creating, abandon ours.
        if state.window.is_some() {
            drop(state);
            warn!("Overlay window created concurrently; discarding duplicate");
            discard_overlay_window(window, Some(action_handler as usize));
            return;
        }
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

        // DEADLOCK PREVENTION: snapshot + drop before AppKit layout calls.
        let snap = OverlaySnapshot::from_state(&state);
        drop(state);

        refresh_action_contract_ui_unlocked(&snap, TranscriptionActionContractMode::Raw, false);
        reset_overlay_to_idle_unlocked(&snap);
        let new_h = resize_overlay_unlocked(&snap);
        {
            let mut state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
            state.last_applied_height = new_h;
            state.last_layout_resize_at = Instant::now();
            state.pending_layout_resize = false;
        }

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
    let snap = {
        let state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
        OverlaySnapshot::from_state(&state)
    };
    set_status_message_unlocked(&snap, status, true);
}

/// Append a delta (streaming token) to the overlay text
pub fn append_transcription_delta(delta: &str) {
    let delta_owned = delta.to_string();
    Queue::main().exec_async(move || {
        append_transcription_delta_impl(&delta_owned);
    });
}

fn append_transcription_delta_impl(delta: &str) {
    // Extract text + snapshot under lock, then drop before AppKit calls.
    let (visible_text, snap, needs_resize) = {
        let mut state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
        let len_before = state.accumulated_text.len();
        codescribe_core::pipeline::contracts::TranscriptDelta::from_raw(delta)
            .apply(&mut state.accumulated_text);
        let len_after = state.accumulated_text.len();
        let visible = display_text_for_state(&state);
        let snap = OverlaySnapshot::from_state(&state);

        // Throttled resize: trigger immediately on structural changes (newlines,
        // backspace/deletion that shortens text), otherwise throttle by time.
        let now = Instant::now();
        let structural_change = delta.contains('\n') || len_after < len_before;
        let needs_resize = structural_change
            || now.duration_since(state.last_layout_resize_at).as_millis()
                >= OVERLAY_LAYOUT_THROTTLE_MS as u128;
        if needs_resize {
            state.last_layout_resize_at = now;
            state.pending_layout_resize = false;
        } else {
            state.pending_layout_resize = true;
        }
        (visible, snap, needs_resize)
    }; // Lock dropped.

    update_overlay_text_unlocked(snap.text_view, &visible_text);
    if needs_resize {
        let new_h = resize_overlay_unlocked(&snap);
        let mut state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
        state.last_applied_height = new_h;
    }
}

/// Set the full text in the overlay
pub fn set_transcription_text(text: &str) {
    let text_owned = text.to_string();
    Queue::main().exec_async(move || {
        set_transcription_text_impl(&text_owned);
    });
}

fn set_transcription_text_impl(text: &str) {
    let (visible_text, snap) = {
        let mut state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
        state.accumulated_text = text.to_string();
        state.last_pass_text = text.to_string();
        let visible = display_text_for_state(&state);
        let snap = OverlaySnapshot::from_state(&state);
        state.last_layout_resize_at = Instant::now();
        state.pending_layout_resize = false;
        (visible, snap)
    }; // Lock dropped.

    update_overlay_text_unlocked(snap.text_view, &visible_text);
    let new_h = resize_overlay_unlocked(&snap);
    {
        let mut state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
        state.last_applied_height = new_h;
    }
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
        let (visible_text, snap, decision_mode) = {
            let mut state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
            state.raw_text = raw_text_owned;
            state.last_pass_text = last_pass_owned;
            state.action_contract_mode = mode_copy;
            let visible = display_text_for_state(&state);
            let dm = state.decision_mode;
            let snap = OverlaySnapshot::from_state(&state);
            state.last_layout_resize_at = Instant::now();
            state.pending_layout_resize = false;
            (visible, snap, dm)
        }; // Lock dropped.

        refresh_action_contract_ui_unlocked(&snap, mode_copy, decision_mode);
        update_overlay_text_unlocked(snap.text_view, &visible_text);
        let new_h = resize_overlay_unlocked(&snap);
        {
            let mut state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
            state.last_applied_height = new_h;
        }
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
    let snap = {
        let mut state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
        state.accumulated_text.clear();
        state.raw_text.clear();
        state.last_pass_text.clear();
        state.action_contract_mode = TranscriptionActionContractMode::Raw;
        state.decision_mode = false;
        state.hover_active = false;
        state.last_layout_resize_at = Instant::now();
        state.pending_layout_resize = false;
        OverlaySnapshot::from_state(&state)
    }; // Lock dropped before AppKit calls.

    update_overlay_text_unlocked(snap.text_view, "");
    let new_h = resize_overlay_unlocked(&snap);
    set_action_buttons_visible_unlocked(&snap, false);
    set_recording_button_visible_unlocked(&snap, false);
    set_auto_hide_hint_visible_unlocked(&snap, TranscriptionActionContractMode::Raw, false);
    if let Some(spinner_ptr) = snap.progress_indicator {
        unsafe {
            set_hidden(spinner_ptr as Id, true);
        }
    }
    reset_overlay_to_idle_unlocked(&snap);

    {
        let mut state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
        state.last_applied_height = new_h;
    }
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
        let (snap, mode) = {
            let state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
            (
                OverlaySnapshot::from_state(&state),
                state.action_contract_mode,
            )
        }; // Lock dropped.
        set_auto_hide_hint_visible_unlocked(&snap, mode, true);
    });

    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_secs(auto_hide_delay_secs()));

        if should_auto_hide(generation) {
            hide_transcription_overlay();
            debug!(
                "Transcription overlay auto-hidden after {}s",
                auto_hide_delay_secs()
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
        let (snap, mode) = {
            let mut state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
            state.decision_mode = true;
            (
                OverlaySnapshot::from_state(&state),
                state.action_contract_mode,
            )
        }; // Lock dropped before AppKit calls.
        set_action_buttons_visible_unlocked(&snap, true);
        set_auto_hide_hint_visible_unlocked(&snap, mode, true);
        set_recording_button_visible_unlocked(&snap, false);
        set_recording_status_unlocked(&snap, false);
    });
}

/// Enter recording mode: hide actions, show recording indicator
pub fn enter_recording_mode() {
    Queue::main().exec_async(|| {
        let (snap, mode) = {
            let mut state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
            state.decision_mode = false;
            state.hover_active = false;
            (
                OverlaySnapshot::from_state(&state),
                state.action_contract_mode,
            )
        }; // Lock dropped before AppKit calls.
        set_action_buttons_visible_unlocked(&snap, false);
        set_auto_hide_hint_visible_unlocked(&snap, mode, false);
        set_recording_button_visible_unlocked(&snap, true);
        // Show recording indicator (red dot + text), no spinner
        set_recording_status_unlocked(&snap, true);
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
        window_discard(window_ptr as Id);
    }
}

fn discard_overlay_window(window: Id, action_handler_ptr: Option<usize>) {
    unsafe {
        window_discard(window);
        if let Some(ptr) = action_handler_ptr {
            release_object(ptr as Id);
        }
    }
}

fn hide_transcription_overlay_impl() {
    // DEADLOCK PREVENTION: extract window_ptr and clear state under lock,
    // then drop lock before the animate_fade AppKit call.
    let (window_ptr, action_handler_ptr) = {
        let mut state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
        let wp = state.window.take();
        let action_handler = state.action_handler.take();
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
        (wp, action_handler)
    }; // Lock dropped.

    if let Some(window_ptr) = window_ptr {
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
                if let Some(ptr) = action_handler_ptr {
                    unsafe {
                        release_object(ptr as Id);
                    }
                }
            });
        });

        debug!("Transcription overlay hidden");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::presentation::emitter::PresentationEmitter;
    use codescribe_core::audio::load_audio_file;
    use codescribe_core::pipeline::contracts::{
        DeltaSink, EngineEvent, EventSink, TranscriptDelta,
    };
    use codescribe_core::pipeline::streaming::collect_buffered_engine_events;
    use serial_test::serial;
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::sync::{Arc, Mutex as StdMutex};
    use tokio::sync::Mutex;

    const OVERLAY_REAL_FLOW_OPT_IN_ENV: &str = "CODESCRIBE_E2E_STT";

    fn overlay_real_flow_enabled() -> bool {
        std::env::var(OVERLAY_REAL_FLOW_OPT_IN_ENV)
            .ok()
            .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
            .unwrap_or(false)
    }

    fn canonical_data_assets_dir() -> Option<PathBuf> {
        let home = std::env::var("HOME").ok()?;
        let dir = PathBuf::from(home).join(".codescribe/data_assets");
        if dir.exists() { Some(dir) } else { None }
    }

    fn canonical_overlay_cases() -> Vec<(PathBuf, PathBuf)> {
        let Some(dir) = canonical_data_assets_dir() else {
            return Vec::new();
        };

        let mut out = Vec::new();
        let entries = match fs::read_dir(&dir) {
            Ok(entries) => entries,
            Err(_) => return Vec::new(),
        };

        for entry in entries.flatten() {
            let wav = entry.path();
            if wav.extension().and_then(|ext| ext.to_str()) != Some("wav") {
                continue;
            }

            let Some(stem) = wav.file_stem().and_then(|stem| stem.to_str()) else {
                continue;
            };
            let reference = dir.join(format!(
                "{stem}_codescribe_raw_human_transcription_from_wav.txt"
            ));
            if reference.exists() {
                out.push((wav, reference));
            }
        }

        out.sort();
        out
    }

    fn append_utterance_text(rendered: &mut String, text: &str) {
        let trimmed = text.trim();
        if trimmed.is_empty() {
            return;
        }
        if !rendered.is_empty() {
            rendered.push(' ');
        }
        rendered.push_str(trimmed);
    }

    fn final_transcript_from_events(events: &[EngineEvent]) -> String {
        let mut transcript = String::new();
        for event in events {
            if let EngineEvent::UtteranceFinal { text, .. } = event {
                append_utterance_text(&mut transcript, text);
            }
        }
        transcript
    }

    fn normalize_overlay_text(text: &str) -> String {
        text.split_whitespace().collect::<Vec<_>>().join(" ")
    }

    fn human_reference_excerpt(path: &Path) -> String {
        fs::read_to_string(path)
            .map(|text| {
                text.split_whitespace()
                    .take(24)
                    .collect::<Vec<_>>()
                    .join(" ")
            })
            .unwrap_or_default()
    }

    fn reset_overlay_state_for_test() {
        AUTO_HIDE_PENDING.store(false, Ordering::SeqCst);
        AUTO_HIDE_GENERATION.store(0, Ordering::SeqCst);

        let mut state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
        state.window = None;
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
        state.raw_text.clear();
        state.last_pass_text.clear();
        state.accumulated_text.clear();
        state.window_width = OVERLAY_WINDOW_WIDTH;
        state.min_height = OVERLAY_WINDOW_MIN_HEIGHT;
        state.max_height = OVERLAY_WINDOW_MIN_HEIGHT;
        state.last_applied_height = OVERLAY_WINDOW_MIN_HEIGHT;
        state.last_layout_resize_at = Instant::now();
        state.pending_layout_resize = false;
    }

    fn overlay_visible_text_now() -> String {
        let state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
        display_text_for_state(&state)
    }

    fn has_one_to_three_word_collapse(snapshots: &[String]) -> bool {
        let mut saw_substantial_text = false;

        for snapshot in snapshots {
            let words = snapshot.split_whitespace().count();
            if words >= 6 || snapshot.chars().count() >= 30 {
                saw_substantial_text = true;
            }

            if saw_substantial_text && (1..=3).contains(&words) {
                return true;
            }
        }

        false
    }

    struct OverlayReplaySink {
        snapshots: Arc<StdMutex<Vec<String>>>,
    }

    impl DeltaSink for OverlayReplaySink {
        fn apply(&self, delta: &TranscriptDelta) {
            append_transcription_delta_impl(&delta.delta);
            let visible = overlay_visible_text_now();
            self.snapshots
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .push(visible);
        }
    }

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
        assert_eq!(
            parse_auto_hide_delay_secs(None),
            DEFAULT_AUTO_HIDE_DELAY_SECS
        );
        assert_eq!(
            parse_auto_hide_delay_secs(Some("2")),
            MIN_AUTO_HIDE_DELAY_SECS
        );
        assert_eq!(
            parse_auto_hide_delay_secs(Some("999")),
            MAX_AUTO_HIDE_DELAY_SECS
        );
        assert_eq!(parse_auto_hide_delay_secs(Some("18")), 18);
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
    fn test_action_text_ai_mode_returns_empty_when_last_pass_empty() {
        let mut state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
        state.action_contract_mode = TranscriptionActionContractMode::AiFormat;
        state.raw_text = "raw transcript".to_string();
        state.accumulated_text = "overlay preview".to_string();
        state.last_pass_text.clear();

        let text = action_text_for_contract(&state);
        assert!(text.is_empty());
    }

    #[test]
    #[serial]
    fn test_display_text_prefers_live_preview_over_action_contract() {
        let mut state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
        state.decision_mode = true;
        state.action_contract_mode = TranscriptionActionContractMode::AiFormat;
        state.raw_text = "raw transcript".to_string();
        state.accumulated_text = "overlay preview".to_string();
        state.last_pass_text = "final last-pass".to_string();

        let text = display_text_for_state(&state);
        assert_eq!(text, "overlay preview");
    }

    #[test]
    #[serial]
    fn test_display_text_falls_back_to_action_contract_when_preview_empty() {
        let mut state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
        state.decision_mode = true;
        state.action_contract_mode = TranscriptionActionContractMode::AiFormat;
        state.raw_text = "raw transcript".to_string();
        state.accumulated_text.clear();
        state.last_pass_text = "final last-pass".to_string();

        let text = display_text_for_state(&state);
        assert_eq!(text, "final last-pass");
    }

    #[test]
    fn test_stable_overlay_preview_text_keeps_complete_tail() {
        let text = "To jest stabilne zdanie.";
        assert_eq!(stable_overlay_preview_text(text), text);
    }

    #[test]
    fn test_stable_overlay_preview_text_trims_partial_tail_word() {
        let text = "To jest stabilne zda";
        assert_eq!(stable_overlay_preview_text(text), "To jest stabilne ");
    }

    #[test]
    fn test_stable_overlay_preview_text_without_boundary_returns_text() {
        assert_eq!(stable_overlay_preview_text("partial"), "partial");
    }

    #[test]
    fn test_overlay_visible_text_decision_mode_uses_exact_text() {
        let text = "pełny tekst kontraktu bez trimowania";
        assert_eq!(overlay_visible_text(text, true), text);
    }

    #[test]
    fn test_overlay_visible_text_live_mode_defaults_to_exact_text() {
        let text = "To jest stabilne zda";
        assert_eq!(overlay_visible_text(text, false), text);
    }

    #[test]
    fn transcription_overlay_source_uses_shared_tafla_window_contract() {
        let source = include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/app/ui/overlay/mod.rs"
        ));
        let overlay_impl = source
            .split("fn show_transcription_overlay_impl()")
            .nth(1)
            .expect("transcription overlay impl present");
        assert!(overlay_impl.contains("create_borderless_tafla_window("));
        assert!(overlay_impl.contains("create_tafla_single_shell("));
        assert!(!overlay_impl.contains("create_glass_effect_view_with("));
        assert!(!overlay_impl.contains("set_glass_effect_content_view("));
    }

    #[tokio::test]
    #[serial]
    async fn overlay_real_flow_from_canonical_assets_never_collapses_to_1_3_words() {
        if !overlay_real_flow_enabled() {
            eprintln!(
                "Skipping overlay real-flow E2E (set {}=1 to enable)",
                OVERLAY_REAL_FLOW_OPT_IN_ENV
            );
            return;
        }

        if let Err(err) = codescribe_core::stt::whisper::singleton::get_model_path() {
            eprintln!("Skipping overlay real-flow E2E: local Whisper model unavailable: {err}");
            return;
        }

        let cases = canonical_overlay_cases();
        if cases.is_empty() {
            eprintln!("Skipping overlay real-flow E2E: no canonical data assets found");
            return;
        }

        let previous_stable_preview = std::env::var("CODESCRIBE_OVERLAY_STABLE_PREVIEW").ok();
        unsafe {
            std::env::set_var("CODESCRIBE_OVERLAY_STABLE_PREVIEW", "0");
        }

        for (audio_path, reference_path) in cases {
            reset_overlay_state_for_test();

            let (samples, sample_rate) =
                load_audio_file(&audio_path).expect("load canonical audio asset");
            let events =
                collect_buffered_engine_events(&samples, sample_rate, Some("pl".to_string()))
                    .await
                    .expect("collect engine events for overlay replay");

            assert!(
                events
                    .iter()
                    .any(|event| matches!(event, EngineEvent::Preview { .. })),
                "expected Preview events for canonical asset {}",
                audio_path.display()
            );

            let expected_final = final_transcript_from_events(&events);
            assert!(
                !expected_final.trim().is_empty(),
                "expected non-empty final transcript for {}",
                audio_path.display()
            );

            let transcript_buffer = Arc::new(Mutex::new(String::new()));
            let snapshots = Arc::new(StdMutex::new(Vec::<String>::new()));
            let sink: Arc<dyn DeltaSink> = Arc::new(OverlayReplaySink {
                snapshots: Arc::clone(&snapshots),
            });
            let mut emitter = PresentationEmitter::new(transcript_buffer, Some(sink), None);

            for event in &events {
                emitter.on_event(event);
            }
            emitter.finish().await;

            let final_visible = overlay_visible_text_now();
            let snapshot_list = snapshots.lock().unwrap_or_else(|e| e.into_inner()).clone();

            assert!(
                !snapshot_list.is_empty(),
                "expected visible overlay snapshots for {}",
                audio_path.display()
            );
            assert!(
                !has_one_to_three_word_collapse(&snapshot_list),
                "overlay collapsed to 1-3 words for {} (reference excerpt: {})",
                audio_path.display(),
                human_reference_excerpt(&reference_path)
            );
            assert_eq!(
                normalize_overlay_text(&final_visible),
                normalize_overlay_text(&expected_final),
                "overlay final visible transcript diverged for {}",
                audio_path.display()
            );
        }

        match previous_stable_preview {
            Some(value) => unsafe { std::env::set_var("CODESCRIBE_OVERLAY_STABLE_PREVIEW", value) },
            None => unsafe { std::env::remove_var("CODESCRIBE_OVERLAY_STABLE_PREVIEW") },
        }
        reset_overlay_state_for_test();
    }
}
