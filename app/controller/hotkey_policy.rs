//! Pure hotkey / stop-path policy decisions (no controller state).

use std::time::Duration;

use super::types::{HotkeyAction, HotkeyInput, HotkeyType, State};
use crate::os::hotkeys::HoldMode;

/// Assistive hold bindings keep a 400ms floor even if settings lower the
/// generic hold delay — prevents accidental Emil sessions on short taps.
const ASSISTIVE_HOLD_START_DELAY_FLOOR_MS: u64 = 400;

/// How long a stop caller may wait for adjudication to settle.
pub(super) const STOP_TIMEOUT: Duration = Duration::from_secs(120);

/// Apply the assistive floor to a configured hold delay.
///
/// See `ASSISTIVE_HOLD_START_DELAY_FLOOR_MS` for why assistive holds ignore a
/// lower configured value.
pub(super) fn effective_hold_start_delay_ms(configured_ms: u64, assistive: bool) -> u64 {
    if assistive {
        configured_ms.max(ASSISTIVE_HOLD_START_DELAY_FLOOR_MS)
    } else {
        configured_ms
    }
}

/// Whether the toggle stop path runs a Whisper final pass (`CODESCRIBE_TOGGLE_FINAL_PASS`).
///
/// Defaults to enabled; only an explicit falsey value (`0`, `false`, `no`, `off`,
/// or empty) turns it off.
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

/// Whether this stop should go through adjudication rather than the plain stop path.
///
/// Only a non-assistive toggle recording with the final pass enabled qualifies —
/// assistive turns deliver live and have no Whisper pass to adjudicate against.
pub(super) fn should_use_toggle_adjudicated_stop(
    current_state: State,
    assistive: bool,
    toggle_final_pass: bool,
) -> bool {
    current_state == State::RecToggle && !assistive && toggle_final_pass
}

/// Whether an event's mode flags (assistive, …) should overwrite the current ones.
///
/// A toggle press that *stops* an in-progress toggle recording is excluded: the
/// stop must not retroactively change the mode the recording started in.
///
/// A hold *Press* (legacy mid-hold `HoldUpdate`) never flips destination.
/// Destination is latched at hold-down; Shift/Command attach `{selection_N}`
/// through `RecordingController::attach_hold_selection` instead.
pub(super) fn should_apply_incoming_mode_flags(current_state: State, event: &HotkeyInput) -> bool {
    if event.key_type == HotkeyType::Hold && event.action == HotkeyAction::Press {
        return false;
    }
    matches!(event.action, HotkeyAction::Down | HotkeyAction::Press)
        && !(event.key_type == HotkeyType::Toggle && current_state == State::RecToggle)
}

/// Whether this input begins a capture: a hold going down, or a toggle /
/// conversation key being pressed.
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

/// Whether a dictation hold that outlived its start delay must stay closed
/// because one of Codescribe's own message fields owns the caret.
///
/// That field carries its own microphone, which records into the field and
/// its conversation. A global dictation take would open the dictation overlay
/// over it and deliver somewhere else, so the hold is refused and the field
/// is asked to point at its microphone instead. Assistive holds (Chat /
/// Selection) target the agent, not the focused field, and stay untouched.
pub(super) fn hold_start_redirects_to_composer(
    assistive: bool,
    hold_mode: HoldMode,
    composer_caret_owned: bool,
) -> bool {
    composer_caret_owned && !assistive && matches!(hold_mode, HoldMode::Raw)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dictation_hold_redirects_only_while_a_composer_owns_the_caret() {
        assert!(hold_start_redirects_to_composer(false, HoldMode::Raw, true));
        assert!(!hold_start_redirects_to_composer(
            false,
            HoldMode::Raw,
            false
        ));
    }

    #[test]
    fn assistive_holds_never_redirect_to_the_composer() {
        assert!(!hold_start_redirects_to_composer(true, HoldMode::Raw, true));
        assert!(!hold_start_redirects_to_composer(
            false,
            HoldMode::Chat,
            true
        ));
        assert!(!hold_start_redirects_to_composer(
            false,
            HoldMode::Selection,
            true
        ));
    }
}
