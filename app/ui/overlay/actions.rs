//! Objective-C action-handler bridge: button callbacks (Copy / Agent /
//! Format / Finish) and hover tracking, plus the action-contract text
//! snapshot used by the controller's commit path.

use std::sync::Once;
use std::sync::atomic::Ordering;

use objc::declare::ClassDecl;
use objc::runtime::{Class, Object, Sel};
use objc::{msg_send, sel, sel_impl};
use tracing::{info, warn};

use super::lifecycle::{hide_transcription_overlay, schedule_auto_hide};
#[cfg(test)]
use super::state::TranscriptionOverlayState;
use super::state::{
    AUTO_HIDE_GENERATION, AUTO_HIDE_PENDING, FormatPhase, OVERLAY_STATE, OverlaySnapshot,
    action_text_for_contract, apply_user_edit_to_state,
};
use super::widgets::{set_action_buttons_visible_unlocked, set_status_message_unlocked};
use crate::os::clipboard;
use crate::ui_helpers::{Id, get_text_view_string};

static ACTION_HANDLER_INIT: Once = Once::new();
static mut ACTION_HANDLER_CLASS: *const Class = std::ptr::null();

#[cfg(test)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum AugmentAction {
    CommitLiveSegment,
    HandoffDecisionText(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum OverlayActionButtonRole {
    FormatPaste,
    Copy,
    AgentClose,
    Finish,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum OverlayButtonAction {
    Format,
    Copy,
    Agent,
    Close,
    Finish,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct OverlayButtonRoute {
    pub(super) action: OverlayButtonAction,
    pub(super) selector_name: &'static str,
}

pub(super) const SETTINGS_SELECTOR_NAME: &str = "onTabSettings:";

pub(super) fn overlay_button_route(
    role: OverlayActionButtonRole,
    phase: FormatPhase,
) -> OverlayButtonRoute {
    match role {
        OverlayActionButtonRole::FormatPaste => match phase {
            FormatPhase::Formatted => OverlayButtonRoute {
                action: OverlayButtonAction::Copy,
                selector_name: "onCopyTranscript:",
            },
            FormatPhase::Idle | FormatPhase::Formatting => OverlayButtonRoute {
                action: OverlayButtonAction::Format,
                selector_name: "onFormatTranscript:",
            },
        },
        OverlayActionButtonRole::Copy => match phase {
            FormatPhase::Formatted => OverlayButtonRoute {
                action: OverlayButtonAction::Agent,
                selector_name: "onAgentTranscript:",
            },
            FormatPhase::Idle | FormatPhase::Formatting => OverlayButtonRoute {
                action: OverlayButtonAction::Copy,
                selector_name: "onCopyTranscript:",
            },
        },
        OverlayActionButtonRole::AgentClose => match phase {
            FormatPhase::Formatted => OverlayButtonRoute {
                action: OverlayButtonAction::Close,
                selector_name: "onCloseTranscript:",
            },
            FormatPhase::Idle | FormatPhase::Formatting => OverlayButtonRoute {
                action: OverlayButtonAction::Agent,
                selector_name: "onAgentTranscript:",
            },
        },
        OverlayActionButtonRole::Finish => OverlayButtonRoute {
            action: OverlayButtonAction::Finish,
            selector_name: "onCommitRecording:",
        },
    }
}

pub(super) fn overlay_button_selector(role: OverlayActionButtonRole, phase: FormatPhase) -> Sel {
    let route = overlay_button_route(role, phase);
    debug_assert_ne!(route.selector_name, SETTINGS_SELECTOR_NAME);
    match route.action {
        OverlayButtonAction::Format => sel!(onFormatTranscript:),
        OverlayButtonAction::Copy => sel!(onCopyTranscript:),
        OverlayButtonAction::Agent => sel!(onAgentTranscript:),
        OverlayButtonAction::Close => sel!(onCloseTranscript:),
        OverlayButtonAction::Finish => sel!(onCommitRecording:),
    }
}

pub(super) fn action_handler_class() -> *const Class {
    ACTION_HANDLER_INIT.call_once(|| unsafe {
        let superclass = Class::get("NSObject").unwrap();
        let mut decl = ClassDecl::new("TranscriptionOverlayActionHandler", superclass).unwrap();

        decl.add_method(
            sel!(onCopyTranscript:),
            on_copy_transcript as extern "C" fn(&Object, Sel, Id),
        );
        decl.add_method(
            sel!(onAgentTranscript:),
            on_agent_transcript as extern "C" fn(&Object, Sel, Id),
        );
        decl.add_method(
            sel!(onFormatTranscript:),
            on_format_transcript as extern "C" fn(&Object, Sel, Id),
        );
        decl.add_method(
            sel!(onPasteTranscript:),
            on_paste_transcript as extern "C" fn(&Object, Sel, Id),
        );
        decl.add_method(
            sel!(onCloseTranscript:),
            on_close_transcript as extern "C" fn(&Object, Sel, Id),
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
        decl.add_method(
            sel!(textDidChange:),
            on_text_did_change as extern "C" fn(&Object, Sel, Id),
        );

        ACTION_HANDLER_CLASS = decl.register();
    });
    unsafe { ACTION_HANDLER_CLASS }
}

fn current_action_text_snapshot() -> (String, bool, OverlaySnapshot) {
    let (fallback, decision_mode, snap) = {
        let state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
        (
            action_text_for_contract(&state),
            state.decision_mode,
            OverlaySnapshot::from_state(&state),
        )
    };

    if decision_mode && let Some(text_view_ptr) = snap.text_view {
        let edited = unsafe { get_text_view_string(text_view_ptr as Id) };
        return (edited, decision_mode, snap);
    }

    (fallback, decision_mode, snap)
}

#[cfg(test)]
pub(super) fn augment_action_for_state(state: &TranscriptionOverlayState) -> Option<AugmentAction> {
    let text = action_text_for_contract(state);
    if text.trim().is_empty() {
        return None;
    }
    if state.decision_mode {
        Some(AugmentAction::HandoffDecisionText(text))
    } else {
        Some(AugmentAction::CommitLiveSegment)
    }
}

/// Returns the current action-contract text (Raw or AiFormat depending on
/// `state.action_contract_mode`). Used by controller's `commit_segment` to
/// read segment text for save without coupling button handlers to controller
/// state. Returns empty string if overlay state lock is poisoned (recoverable).
pub fn current_segment_text() -> String {
    let (text, _, _) = current_action_text_snapshot();
    text
}

/// Handler: Copy transcript using contract source of truth.
extern "C" fn on_copy_transcript(_this: &Object, _cmd: Sel, _sender: Id) {
    let (text, _, snap) = current_action_text_snapshot();
    if text.is_empty() {
        return;
    }
    if let Err(e) = clipboard::set_clipboard(&text) {
        warn!("Failed to copy transcript: {}", e);
        set_status_message_unlocked(&snap, "Copy failed", true);
        return;
    }

    info!("Copied transcript ({} chars)", text.len());
    if snap.format_phase == FormatPhase::Formatted {
        set_status_message_unlocked(&snap, "Copied", false);
    } else {
        hide_transcription_overlay();
    }
}

/// Handler: Agent = hand the whole transcript to the Agent (Emil).
///
/// Decision-mode (post-recording): hands off the complete session transcript to
/// the voice-chat overlay as a single message. Live (mid-recording, legacy) clips
/// and commits the current segment, then augments. ADR 2026-05-28 Faza 1 renames
/// the former "Augment" action to "Agent" — same handoff, clearer contract.
extern "C" fn on_agent_transcript(_this: &Object, _cmd: Sel, _sender: Id) {
    let (text, decision_mode, _) = current_action_text_snapshot();
    if text.trim().is_empty() {
        return;
    }

    if decision_mode {
        crate::ui::voice_chat::show_voice_chat_overlay();
        crate::ui::voice_chat::show_agent_tab();
        crate::ui::voice_chat::handoff_transcript_to_chat(&text);
    } else {
        crate::controller::request_segment_commit_and_augment();
    }
    hide_transcription_overlay();
}

/// Handler: Format = run AI formatting on the decision transcript in-place.
///
/// ADR 2026-05-28 Faza 1: formatting is a post-recording CHOICE, not something the
/// dictation does mid-stream. Revision 2026-06-11 keeps the overlay open: Format
/// enters a disabled "Formatting..." phase, then returns editable text for
/// Copy / Agent / Close.
extern "C" fn on_format_transcript(_this: &Object, _cmd: Sel, _sender: Id) {
    let (text, _, snap) = current_action_text_snapshot();
    if text.trim().is_empty() {
        return;
    }

    match snap.format_phase {
        FormatPhase::Formatting => {}
        FormatPhase::Formatted => {
            on_copy_transcript(_this, _cmd, _sender);
        }
        FormatPhase::Idle => {
            crate::ui::overlay::enter_overlay_formatting();
            crate::controller::request_format_for_overlay(text, |formatted_text| {
                crate::ui::overlay::apply_overlay_format_result(&formatted_text);
            });
        }
    }
}

/// Handler: Paste = paste the current editable formatted transcript.
extern "C" fn on_paste_transcript(_this: &Object, _cmd: Sel, _sender: Id) {
    let (text, _, snap) = current_action_text_snapshot();
    if text.trim().is_empty() {
        return;
    }

    crate::controller::request_overlay_paste(text);
    set_status_message_unlocked(&snap, "Pasted", false);
}

/// Handler: Close = dismiss the formatted overlay without routing through Agent.
extern "C" fn on_close_transcript(_this: &Object, _cmd: Sel, _sender: Id) {
    hide_transcription_overlay();
}

/// Handler: Commit segment = save WAV + transcript + Quick Notes WITHOUT
/// stopping the recorder. Recording continues; buffer offset advances so the
/// next segment starts from here. Overlay fades out.
extern "C" fn on_commit_recording(_this: &Object, _cmd: Sel, _sender: Id) {
    crate::controller::request_segment_commit();
    hide_transcription_overlay();
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
    let (decision_mode, format_phase, snap) = {
        let mut state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
        state.hover_active = false;
        (
            state.decision_mode,
            state.format_phase,
            OverlaySnapshot::from_state(&state),
        )
    }; // Lock dropped before AppKit calls.
    if decision_mode {
        set_action_buttons_visible_unlocked(&snap, true);
        if format_phase == FormatPhase::Idle {
            schedule_auto_hide();
        }
    } else {
        set_action_buttons_visible_unlocked(&snap, false);
    }
}

extern "C" fn on_text_did_change(_this: &Object, _cmd: Sel, notification: Id) {
    let text_view = unsafe {
        let from_notification: Id = if notification.is_null() {
            std::ptr::null_mut()
        } else {
            msg_send![notification, object]
        };
        if !from_notification.is_null() {
            from_notification
        } else {
            let state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
            state
                .text_view
                .map(|ptr| ptr as Id)
                .unwrap_or(std::ptr::null_mut())
        }
    };
    if text_view.is_null() {
        return;
    }

    let edited_text = unsafe { get_text_view_string(text_view) };
    let snap = {
        let mut state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
        apply_user_edit_to_state(&mut state, edited_text);
        OverlaySnapshot::from_state(&state)
    };
    AUTO_HIDE_GENERATION.fetch_add(1, Ordering::SeqCst);
    AUTO_HIDE_PENDING.store(false, Ordering::SeqCst);
    set_status_message_unlocked(&snap, "Edited", false);
}
