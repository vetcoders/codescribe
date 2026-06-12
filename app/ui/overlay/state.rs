//! Overlay state cell, AppKit pointer snapshot, auto-hide bookkeeping, and
//! the action-contract source-of-truth selection.

use std::sync::atomic::AtomicBool;
use std::sync::{Mutex, OnceLock};
use std::time::Instant;

use super::layout::OVERLAY_WINDOW_MIN_HEIGHT;

// Auto-hide delay after recording completes (configurable via env)
pub(super) const DEFAULT_AUTO_HIDE_DELAY_SECS: u64 = 15;
pub(super) const MIN_AUTO_HIDE_DELAY_SECS: u64 = 3;
pub(super) const MAX_AUTO_HIDE_DELAY_SECS: u64 = 60;

pub(super) fn parse_auto_hide_delay_secs(raw: Option<&str>) -> u64 {
    raw.and_then(|v| v.parse::<u64>().ok())
        .map(|v| v.clamp(MIN_AUTO_HIDE_DELAY_SECS, MAX_AUTO_HIDE_DELAY_SECS))
        .unwrap_or(DEFAULT_AUTO_HIDE_DELAY_SECS)
}

pub(super) fn auto_hide_delay_secs() -> u64 {
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum FormatPhase {
    Idle,
    Formatting,
    Formatted,
}

/// Transcription overlay state
pub(super) struct TranscriptionOverlayState {
    pub(super) window: Option<usize>,
    pub(super) header_label: Option<usize>,
    pub(super) text_scroll_view: Option<usize>,
    pub(super) text_view: Option<usize>,
    pub(super) status_field: Option<usize>,
    pub(super) auto_hide_label: Option<usize>,
    pub(super) blur_view: Option<usize>,
    pub(super) copy_button: Option<usize>,
    pub(super) augment_button: Option<usize>,
    pub(super) save_button: Option<usize>,
    pub(super) commit_button: Option<usize>,
    pub(super) progress_indicator: Option<usize>,
    pub(super) tracking_area: Option<usize>,
    pub(super) decision_mode: bool,
    pub(super) hover_active: bool,
    pub(super) action_handler: Option<usize>,
    pub(super) action_contract_mode: TranscriptionActionContractMode,
    pub(super) format_phase: FormatPhase,
    pub(super) display_status: String,
    pub(super) raw_text: String,
    pub(super) last_pass_text: String,
    pub(super) accumulated_text: String,
    pub(super) min_height: f64,
    pub(super) max_height: f64,
    pub(super) last_applied_height: f64,
    pub(super) last_layout_resize_at: Instant,
    pub(super) pending_layout_resize: bool,
}

lazy_static::lazy_static! {
    pub(super) static ref OVERLAY_STATE: Mutex<TranscriptionOverlayState> = Mutex::new(TranscriptionOverlayState {
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
        format_phase: FormatPhase::Idle,
        display_status: String::new(),
        raw_text: String::new(),
        last_pass_text: String::new(),
        accumulated_text: String::new(),
        min_height: OVERLAY_WINDOW_MIN_HEIGHT,
        max_height: OVERLAY_WINDOW_MIN_HEIGHT,
        last_applied_height: OVERLAY_WINDOW_MIN_HEIGHT,
        last_layout_resize_at: Instant::now(),
        pending_layout_resize: false,
    });
}

/// Snapshot of widget pointers + layout params for AppKit calls outside lock scope.
///
/// DEADLOCK PREVENTION: extract this while holding `OVERLAY_STATE`, then
/// **drop the lock** before using the pointers in AppKit `msg_send!` calls.
/// AppKit can spin a nested run-loop during `setFrame:display:`, `orderFront:`,
/// etc., and pending `Queue::main().exec_async` blocks that also lock
/// `OVERLAY_STATE` will deadlock on the non-reentrant `std::sync::Mutex`.
#[derive(Clone)]
pub(super) struct OverlaySnapshot {
    pub(super) window: Option<usize>,
    pub(super) header_label: Option<usize>,
    pub(super) text_scroll_view: Option<usize>,
    pub(super) text_view: Option<usize>,
    pub(super) status_field: Option<usize>,
    pub(super) auto_hide_label: Option<usize>,
    pub(super) blur_view: Option<usize>,
    pub(super) copy_button: Option<usize>,
    pub(super) augment_button: Option<usize>,
    pub(super) save_button: Option<usize>,
    pub(super) commit_button: Option<usize>,
    pub(super) progress_indicator: Option<usize>,
    pub(super) action_handler: Option<usize>,
    pub(super) format_phase: FormatPhase,
    pub(super) display_status: String,
    pub(super) min_height: f64,
    pub(super) max_height: f64,
    pub(super) last_applied_height: f64,
}

impl OverlaySnapshot {
    pub(super) fn from_state(state: &TranscriptionOverlayState) -> Self {
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
            action_handler: state.action_handler,
            format_phase: state.format_phase,
            display_status: state.display_status.clone(),
            min_height: state.min_height,
            max_height: state.max_height,
            last_applied_height: state.last_applied_height,
        }
    }
}

/// Flag to track if auto-hide timer is pending
pub(super) static AUTO_HIDE_PENDING: AtomicBool = AtomicBool::new(false);
/// Counter to invalidate old timers
pub(super) static AUTO_HIDE_GENERATION: std::sync::atomic::AtomicU64 =
    std::sync::atomic::AtomicU64::new(0);

pub(super) fn action_text_for_contract(state: &TranscriptionOverlayState) -> String {
    match state.action_contract_mode {
        TranscriptionActionContractMode::Raw => state.raw_text.clone(),
        TranscriptionActionContractMode::AiFormat => state.last_pass_text.clone(),
    }
}
