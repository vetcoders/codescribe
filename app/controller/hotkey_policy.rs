//! Pure hotkey / stop-path policy decisions (no controller state).

use std::time::Duration;

use super::types::{HotkeyAction, HotkeyInput, HotkeyType, State};

/// Assistive hold bindings keep a 400ms floor even if settings lower the
/// generic hold delay — prevents accidental Emil sessions on short taps.
const ASSISTIVE_HOLD_START_DELAY_FLOOR_MS: u64 = 400;

pub(super) const TOGGLE_STOP_ADJUDICATE_TIMEOUT: Duration = Duration::from_secs(120);
pub(super) const STOP_TIMEOUT: Duration = TOGGLE_STOP_ADJUDICATE_TIMEOUT;

pub(super) fn effective_hold_start_delay_ms(configured_ms: u64, assistive: bool) -> u64 {
    if assistive {
        configured_ms.max(ASSISTIVE_HOLD_START_DELAY_FLOOR_MS)
    } else {
        configured_ms
    }
}

#[cfg(test)]
pub(super) fn toggle_stop_adjudicate_timeout() -> Duration {
    STOP_TIMEOUT
}

pub(super) fn toggle_final_pass_enabled() -> bool {
    std::env::var("CODESCRIBE_TOGGLE_FINAL_PASS")
        .ok()
        .map(|v| {
            !matches!(
                v.trim().to_ascii_lowercase().as_str(),
                "" | "0" | "false" | "no" | "off"
            )
        })
        .unwrap_or(true)
}

pub(super) fn should_use_toggle_adjudicated_stop(
    current_state: State,
    assistive: bool,
    toggle_final_pass: bool,
) -> bool {
    current_state == State::RecToggle && !assistive && toggle_final_pass
}

pub(super) fn should_apply_incoming_mode_flags(current_state: State, event: &HotkeyInput) -> bool {
    matches!(event.action, HotkeyAction::Down | HotkeyAction::Press)
        && !(event.key_type == HotkeyType::Toggle && current_state == State::RecToggle)
}

pub(super) fn is_hotkey_start_event(event: &HotkeyInput) -> bool {
    matches!(
        (event.key_type, event.action),
        (HotkeyType::Hold, HotkeyAction::Down)
            | (HotkeyType::Toggle, HotkeyAction::Press)
            | (HotkeyType::Conversation, HotkeyAction::Press)
    )
}

/// An assistive *start* hotkey — FN+Shift hold-down, an assistive toggle press,
/// or any start event flagged `assistive` (Chat / Selection / assistive toggle).
/// These are the "Talk Anytime" inputs the user fires to add a new voice intent
/// while Emil/the agent is still answering.
pub(super) fn is_assistive_start_event(event: &HotkeyInput) -> bool {
    is_hotkey_start_event(event) && event.assistive
}

/// Block a *new* hotkey start while a previously-dispatched agent turn is still
/// streaming. This fires only at `State::Idle` — the controller has already
/// returned the mic/transcription pipeline; the agent is answering in the
/// background (a detached `tokio::spawn`, see `send_assistive_with_agent_runtime`).
///
/// Exception — **Assistive Talk Anytime**: assistive start events are allowed
/// through so the user can record a *new* voice intent while the agent answers.
/// The resulting utterance is captured into the existing pending-follow-up
/// buffer (`should_capture_pending_followup` → `get_or_create_pending_followup_index`),
/// not dropped — the living intent grows instead of being ignored. Non-assistive
/// (raw) dictation starts stay blocked: barging a raw transcript into a live
/// agent turn is never wanted, and blocking preserves the single-pipeline
/// guarantee for the dictation path.
///
/// `agent_send_in_flight` is passed in (rather than read from the global) so the
/// decision is a pure function and unit-testable without touching shared state.
pub(super) fn should_block_hotkey_during_agent_send(
    current_state: State,
    event: &HotkeyInput,
    agent_send_in_flight: bool,
) -> bool {
    current_state == State::Idle
        && agent_send_in_flight
        && is_hotkey_start_event(event)
        && !is_assistive_start_event(event)
}
