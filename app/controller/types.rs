//! Controller types and validation
//!
//! Contains type definitions for the recording controller state machine.

/// Application state enum
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    /// Waiting for user input
    Idle,
    /// Recording in hold-to-talk mode
    RecHold,
    /// Recording in toggle mode
    RecToggle,
    /// Processing transcription and formatting
    Busy,
    /// Full-duplex conversation mode (Moshi)
    ///
    /// In this mode, the app simultaneously:
    /// - Records audio from microphone
    /// - Processes through VAD + Moshi LM
    /// - Plays AI response through speaker
    /// - Supports interruption (user can speak while AI responds)
    Conversation,
}

impl std::fmt::Display for State {
    /// Uppercase log token (`IDLE`, `REC_HOLD`, …); not the IPC lowercase form.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            State::Idle => write!(f, "IDLE"),
            State::RecHold => write!(f, "REC_HOLD"),
            State::RecToggle => write!(f, "REC_TOGGLE"),
            State::Busy => write!(f, "BUSY"),
            State::Conversation => write!(f, "CONVERSATION"),
        }
    }
}

impl State {
    /// Lowercase state token used on the IPC surface, distinct from the
    /// uppercase `Display` form used in logs.
    pub fn to_ipc_str(self) -> &'static str {
        match self {
            State::Idle => "idle",
            State::RecHold => "rec_hold",
            State::RecToggle => "rec_toggle",
            State::Busy => "busy",
            State::Conversation => "conversation",
        }
    }
}

/// Hotkey event types
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HotkeyType {
    /// Hold-to-talk recording gesture.
    Hold,
    /// Toggle start/stop recording gesture.
    Toggle,
    /// Full-duplex conversation mode (Ctrl+Option)
    Conversation,
}

/// Hotkey action types
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HotkeyAction {
    /// Physical key-down edge.
    Down,
    /// Physical key-up edge.
    Up,
    /// Discrete press (down+up treated as one).
    Press,
}

/// Complete hotkey event with metadata
#[derive(Debug, Clone)]
pub struct HotkeyInput {
    pub key_type: HotkeyType,
    pub action: HotkeyAction,
    /// Session semantics/destination flag. It never selects a capture,
    /// preview, or final-pass implementation.
    pub assistive: bool,
    pub hold_mode: crate::os::hotkeys::HoldMode,
    pub force_raw: bool,
    pub force_ai: bool,
}

impl HotkeyInput {
    /// Mode token for the `hotkey_decision` line: what kind of take the
    /// gesture would start.
    fn mode_token(&self) -> &'static str {
        if self.assistive {
            "assistive"
        } else if self.force_ai {
            "formatting"
        } else if self.force_raw {
            "raw"
        } else {
            "dictation"
        }
    }

    /// Stable INFO line for one gesture as the controller admits it: gesture
    /// kind, edge, mode, the state it met and the resulting action. Pairs
    /// with the tap's `hotkey_input` journal line, which records the raw key
    /// events that produced the gesture.
    ///
    /// `action` names the admission route only; later gates (Busy, the agent
    /// send gate, a superseded hold generation) still log their own refusal.
    pub(crate) fn decision_line(&self, state: Option<State>, stop: bool) -> String {
        let gesture = match self.key_type {
            HotkeyType::Hold => "hold",
            HotkeyType::Toggle => "toggle",
            HotkeyType::Conversation => "conversation",
        };
        let edge = match self.action {
            HotkeyAction::Down => "down",
            HotkeyAction::Up => "up",
            HotkeyAction::Press => "press",
        };
        let action = if stop {
            "stop_take"
        } else {
            match (self.key_type, self.action, state) {
                (HotkeyType::Toggle, HotkeyAction::Press, Some(State::Idle)) => "start_toggle",
                (HotkeyType::Hold, HotkeyAction::Down, Some(State::Idle)) => "schedule_hold",
                (HotkeyType::Hold, HotkeyAction::Up, _) => "cancel_pending_hold",
                (HotkeyType::Conversation, HotkeyAction::Down, Some(State::Idle)) => {
                    "start_conversation"
                }
                (HotkeyType::Conversation, HotkeyAction::Up, Some(State::Conversation)) => {
                    "stop_conversation"
                }
                _ => "none",
            }
        };
        let state = state.map_or("unknown", State::to_ipc_str);
        format!(
            "hotkey_decision gesture={gesture} edge={edge} mode={} state={state} action={action}",
            self.mode_token()
        )
    }
}

#[cfg(test)]
mod decision_line_tests {
    use super::*;
    use crate::os::hotkeys::HoldMode;

    fn input(key_type: HotkeyType, action: HotkeyAction, force_raw: bool) -> HotkeyInput {
        HotkeyInput {
            key_type,
            action,
            assistive: false,
            hold_mode: HoldMode::Raw,
            force_raw,
            force_ai: false,
        }
    }

    #[test]
    fn decision_line_names_gesture_mode_state_and_action() {
        let fn_tap = input(HotkeyType::Toggle, HotkeyAction::Press, true);
        assert_eq!(
            fn_tap.decision_line(Some(State::Idle), false),
            "hotkey_decision gesture=toggle edge=press mode=raw state=idle action=start_toggle"
        );
        assert_eq!(
            fn_tap.decision_line(Some(State::RecToggle), true),
            "hotkey_decision gesture=toggle edge=press mode=raw state=rec_toggle action=stop_take"
        );
        let hold_down = input(HotkeyType::Hold, HotkeyAction::Down, false);
        assert_eq!(
            hold_down.decision_line(Some(State::Idle), false),
            "hotkey_decision gesture=hold edge=down mode=dictation state=idle action=schedule_hold"
        );
        assert_eq!(
            hold_down.decision_line(None, false),
            "hotkey_decision gesture=hold edge=down mode=dictation state=unknown action=none"
        );
    }
}
