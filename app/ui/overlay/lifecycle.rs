//! Public overlay lifecycle API: status updates, streaming deltas, text and
//! action-contract payloads, mode transitions (recording / processing /
//! decision), auto-hide scheduling, and teardown.
//!
//! Every entry point hops to the main queue and follows the
//! extract-snapshot / drop-lock / AppKit-call pattern documented on
//! `state::OverlaySnapshot`.

use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use dispatch::Queue;
use objc::{msg_send, sel, sel_impl};
use tracing::debug;

use super::layout::{
    OVERLAY_LAYOUT_THROTTLE_MS, OVERLAY_WINDOW_MIN_HEIGHT, resize_overlay_unlocked,
};
use super::preview::display_text_for_state;
use super::state::{
    AUTO_HIDE_GENERATION, AUTO_HIDE_PENDING, FormatPhase, OVERLAY_STATE, OverlaySnapshot,
    TranscriptionActionContractMode, auto_hide_delay_secs,
};
use super::widgets::{
    refresh_action_contract_ui_unlocked, reset_overlay_to_idle_unlocked,
    set_action_buttons_visible_unlocked, set_auto_hide_hint_visible_unlocked,
    set_format_phase_ui_unlocked, set_recording_button_visible_unlocked,
    set_recording_status_unlocked, set_status_message_unlocked, set_text_view_editable_unlocked,
    transcript_text_view_editable, update_overlay_text_unlocked,
};
use crate::ui_helpers::{Id, animate_fade, set_hidden};

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

pub(super) fn append_transcription_delta_impl(delta: &str) {
    // Extract text + snapshot under lock, then drop before AppKit calls.
    let (visible_text, snap, needs_resize) = {
        let mut state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
        if state.user_edited {
            return;
        }
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
        state.user_edited = false;
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
    display_status: String,
) {
    let raw_text_owned = raw_text.to_string();
    let last_pass_owned = last_pass_text.to_string();
    let mode_copy = mode;
    Queue::main().exec_async(move || {
        let (visible_text, snap, decision_mode) = {
            let mut state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
            state.accumulated_text.clear();
            state.raw_text = raw_text_owned;
            state.last_pass_text = last_pass_owned;
            state.user_edited = false;
            state.action_contract_mode = mode_copy;
            state.format_phase = FormatPhase::Idle;
            state.display_status = display_status;
            let visible = display_text_for_state(&state);
            let dm = state.decision_mode;
            let snap = OverlaySnapshot::from_state(&state);
            state.last_layout_resize_at = Instant::now();
            state.pending_layout_resize = false;
            (visible, snap, dm)
        }; // Lock dropped.

        refresh_action_contract_ui_unlocked(&snap, mode_copy, decision_mode);
        set_format_phase_ui_unlocked(&snap, mode_copy);
        set_text_view_editable_unlocked(
            &snap,
            transcript_text_view_editable(decision_mode, snap.format_phase),
        );
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
        state.user_edited = false;
        state.action_contract_mode = TranscriptionActionContractMode::Raw;
        state.format_phase = FormatPhase::Idle;
        state.display_status.clear();
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
    set_text_view_editable_unlocked(&snap, false);
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

pub(super) fn should_auto_hide(expected_generation: u64) -> bool {
    if AUTO_HIDE_GENERATION.load(Ordering::SeqCst) != expected_generation
        || !AUTO_HIDE_PENDING.load(Ordering::SeqCst)
    {
        return false;
    }

    let hovered = {
        let state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
        if state.format_phase == FormatPhase::Formatting {
            return false;
        }
        state.hover_active
    };

    !hovered
}

pub fn enter_overlay_formatting() {
    AUTO_HIDE_GENERATION.fetch_add(1, Ordering::SeqCst);
    AUTO_HIDE_PENDING.store(false, Ordering::SeqCst);

    Queue::main().exec_async(|| {
        let (snap, mode) = {
            let mut state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
            state.format_phase = FormatPhase::Formatting;
            state.display_status = "Formatting...".to_string();
            (
                OverlaySnapshot::from_state(&state),
                state.action_contract_mode,
            )
        };

        set_action_buttons_visible_unlocked(&snap, true);
        set_auto_hide_hint_visible_unlocked(&snap, mode, false);
        set_format_phase_ui_unlocked(&snap, mode);
        set_text_view_editable_unlocked(&snap, false);
        set_status_message_unlocked(&snap, "Formatting...", true);
    });
}

pub fn apply_overlay_format_result(formatted_text: &str) {
    let formatted_text = formatted_text.to_string();
    AUTO_HIDE_GENERATION.fetch_add(1, Ordering::SeqCst);
    AUTO_HIDE_PENDING.store(false, Ordering::SeqCst);

    Queue::main().exec_async(move || {
        let (visible_text, snap, mode) = {
            let mut state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
            state.accumulated_text = formatted_text.clone();
            state.last_pass_text = formatted_text;
            state.user_edited = false;
            state.decision_mode = true;
            state.action_contract_mode = TranscriptionActionContractMode::AiFormat;
            state.format_phase = FormatPhase::Formatted;
            state.display_status = "Formatted".to_string();
            let visible = display_text_for_state(&state);
            let snap = OverlaySnapshot::from_state(&state);
            state.last_layout_resize_at = Instant::now();
            state.pending_layout_resize = false;
            (visible, snap, state.action_contract_mode)
        };

        refresh_action_contract_ui_unlocked(&snap, mode, true);
        set_auto_hide_hint_visible_unlocked(&snap, mode, true);
        set_format_phase_ui_unlocked(&snap, mode);
        set_action_buttons_visible_unlocked(&snap, true);
        set_recording_button_visible_unlocked(&snap, false);
        set_text_view_editable_unlocked(&snap, true);
        set_status_message_unlocked(&snap, "Formatted", false);
        update_overlay_text_unlocked(snap.text_view, &visible_text);
        let new_h = resize_overlay_unlocked(&snap);
        {
            let mut state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
            state.last_applied_height = new_h;
        }
        schedule_auto_hide();
    });
}

/// Enter decision mode: show actions on hover for the current transcript
pub fn enter_decision_mode() {
    Queue::main().exec_async(|| {
        let (snap, mode) = {
            let mut state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
            state.decision_mode = true;
            if state.format_phase != FormatPhase::Formatted {
                state.format_phase = FormatPhase::Idle;
            }
            (
                OverlaySnapshot::from_state(&state),
                state.action_contract_mode,
            )
        }; // Lock dropped before AppKit calls.
        set_action_buttons_visible_unlocked(&snap, true);
        set_auto_hide_hint_visible_unlocked(&snap, mode, true);
        set_format_phase_ui_unlocked(&snap, mode);
        set_recording_button_visible_unlocked(&snap, false);
        set_recording_status_unlocked(&snap, false);
        set_text_view_editable_unlocked(
            &snap,
            transcript_text_view_editable(true, snap.format_phase),
        );
    });
}

/// Enter recording mode: hide actions, show recording indicator
pub fn enter_recording_mode() {
    Queue::main().exec_async(|| {
        let (snap, mode) = {
            let mut state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
            state.decision_mode = false;
            state.hover_active = false;
            state.format_phase = FormatPhase::Idle;
            state.user_edited = false;
            (
                OverlaySnapshot::from_state(&state),
                state.action_contract_mode,
            )
        }; // Lock dropped before AppKit calls.
        set_action_buttons_visible_unlocked(&snap, false);
        set_auto_hide_hint_visible_unlocked(&snap, mode, false);
        set_format_phase_ui_unlocked(&snap, mode);
        set_recording_button_visible_unlocked(&snap, true);
        set_text_view_editable_unlocked(&snap, false);
        // Show recording indicator (red dot + text), no spinner
        set_recording_status_unlocked(&snap, true);
    });
}

/// Enter processing mode: recording has stopped, but final transcription /
/// formatting work is still running.
pub fn enter_processing_mode() {
    Queue::main().exec_async(|| {
        let (snap, mode) = {
            let mut state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
            state.decision_mode = false;
            state.hover_active = false;
            state.format_phase = FormatPhase::Idle;
            state.user_edited = false;
            (
                OverlaySnapshot::from_state(&state),
                state.action_contract_mode,
            )
        }; // Lock dropped before AppKit calls.
        set_action_buttons_visible_unlocked(&snap, false);
        set_auto_hide_hint_visible_unlocked(&snap, mode, false);
        set_format_phase_ui_unlocked(&snap, mode);
        set_recording_button_visible_unlocked(&snap, false);
        set_text_view_editable_unlocked(&snap, false);
        set_status_message_unlocked(&snap, "Thinking", true);
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

/// Closes a window by raw pointer (used for delayed close after animation).
///
/// Sends `release` after `close` because the shared shell policy sets
/// `released_when_closed = false`, so AppKit no longer auto-releases the
/// initial alloc/init retain. Without this the NSWindow itself would leak.
///
/// The teardown sequence is wrapped in an explicit `objc2::rc::autoreleasepool`
/// scope so that autoreleased temporaries spawned during AppKit's
/// `windowWillClose` / `removeFromSuperview` / CoreAnimation cleanup chain
/// drain in-scope, before this function returns. Without the scope,
/// pendingowe autoreleases survive into the next runloop tick's pool pop
/// and can hit pointers freed by this same teardown, producing
/// `EXC_BAD_ACCESS` in `objc_release` during `_CFAutoreleasePoolPop`
/// (observed as SIGSEGV on macOS Tahoe beta, 2026-05-10 and 2026-05-13).
fn close_window_by_ptr(window_ptr: usize) {
    debug!(
        "close_window_by_ptr: tearing down NSWindow (ptr={:#x})",
        window_ptr
    );
    objc2::rc::autoreleasepool(|_pool| unsafe {
        crate::ui_helpers::window_discard(window_ptr as Id);
    });
}

fn hide_transcription_overlay_impl() {
    // Operator's screencast 2026-05-26 (75-89s): "skończyłem dyktować ale i tak czerwono".
    // Overlay teardown is the last surface that touches tray status — no subscriber maps
    // controller State::Idle → TrayStatus::Idle automatically. Reset here so the menu bar
    // indicator returns to green whenever the overlay disappears.
    let _ = crate::tray::update_tray_status(crate::tray::TrayStatus::Idle);

    // DEADLOCK PREVENTION: extract handles and clear state under lock,
    // then drop lock before the animate_fade AppKit call.
    let (
        window_ptr,
        tracking_area_ptr,
        action_handler_ptr,
        text_view_ptr,
        copy_button_ptr,
        augment_button_ptr,
        save_button_ptr,
        commit_button_ptr,
    ) = {
        let mut state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
        let wp = state.window.take();
        let tap = state.tracking_area.take();
        let ahp = state.action_handler.take();
        let tvp = state.text_view.take();
        let cbp = state.copy_button.take();
        let abp = state.augment_button.take();
        let sbp = state.save_button.take();
        let cmp = state.commit_button.take();
        state.header_label = None;
        state.text_scroll_view = None;
        state.status_field = None;
        state.auto_hide_label = None;
        state.blur_view = None;
        state.progress_indicator = None;
        state.decision_mode = false;
        state.hover_active = false;
        state.action_contract_mode = TranscriptionActionContractMode::Raw;
        state.format_phase = FormatPhase::Idle;
        state.user_edited = false;
        state.last_applied_height = OVERLAY_WINDOW_MIN_HEIGHT;
        state.last_layout_resize_at = Instant::now();
        state.pending_layout_resize = false;
        // Note: accumulated_text is NOT cleared here - it's needed for clipboard copy
        (wp, tap, ahp, tvp, cbp, abp, sbp, cmp)
    }; // Lock dropped.

    if let Some(window_ptr) = window_ptr {
        let window = window_ptr as Id;

        // Fade out animation (0.15s)
        unsafe {
            animate_fade(window, 0.0, 0.15);
        }

        // Release the tracking area and the action target before the window
        // tears down. Detach every unretained target first so no queued AppKit
        // control/tracking callback can fire on a freed pointer during fade-out.
        unsafe {
            let nil_target: Id = std::ptr::null_mut();
            if let Some(tv_ptr) = text_view_ptr {
                let _: () = msg_send![tv_ptr as Id, setDelegate: nil_target];
            }
            for button_ptr in [
                copy_button_ptr,
                augment_button_ptr,
                save_button_ptr,
                commit_button_ptr,
            ]
            .into_iter()
            .flatten()
            {
                let button = button_ptr as Id;
                let _: () = msg_send![button, setTarget: nil_target];
            }
            if let Some(ta_ptr) = tracking_area_ptr {
                let ta = ta_ptr as Id;
                let content_view: Id = msg_send![window, contentView];
                if !content_view.is_null() {
                    let _: () = msg_send![content_view, removeTrackingArea: ta];
                }
                let _: () = msg_send![ta, release];
            }
            if let Some(ah_ptr) = action_handler_ptr {
                let _: () = msg_send![ah_ptr as Id, release];
            }
        }

        // Close window after brief delay for animation. `close_window_by_ptr`
        // sends `release` after `close` to balance the alloc/init retain
        // (released_when_closed = false in the shared shell policy).
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(200));
            Queue::main().exec_async(move || {
                close_window_by_ptr(window_ptr);
            });
        });

        debug!("Transcription overlay hidden");
    }
}
