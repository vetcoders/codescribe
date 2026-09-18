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
