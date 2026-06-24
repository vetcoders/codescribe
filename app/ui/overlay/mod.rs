//! Simple transcription overlay for non-assistive modes.
//!
//! This module provides a minimal floating overlay window that:
//! - Shows status during recording (Recording..., Processing...)
//! - Displays live streaming transcription text
//! - Supports explicit decision actions (Save/Copy/Augment) after recording
//! - Auto-hides after recording completion
//!
//! Use this for: Ctrl hold (raw), Left ⌥⌥ toggle (normal)
//! For agent chat conversations, use the `ui::voice_chat` overlay.
//!
//! Design: macOS Tahoe Liquid Glass (NSVisualEffectView, HudWindow material)
//!
//! Module layout (decomposed from a single 2456-LOC file):
//! - [`state`] — overlay state cell, AppKit pointer snapshot, auto-hide
//!   bookkeeping, action-contract source selection
//! - [`preview`] — live-preview text filtering (word-boundary stabilization)
//! - [`layout`] — geometry constants, layout metrics, unlocked window resize
//! - [`widgets`] — unlocked AppKit widget mutators (buttons, tooltips, status)
//! - [`actions`] — Objective-C action-handler bridge for button callbacks
//!   and hover tracking
//! - [`window`] — NSWindow construction and static UI build
//! - [`lifecycle`] — public lifecycle API: deltas, text, modes, auto-hide,
//!   teardown
//!
//! External contract: everything re-exported below stays addressable as
//! `crate::ui::overlay::<item>` — controller, settings, and voice_chat call
//! through this facade.

mod actions;
mod layout;
mod lifecycle;
mod preview;
mod state;
#[cfg(test)]
mod tests;
mod widgets;
mod window;

pub use self::actions::current_segment_text;
pub use self::lifecycle::{
    append_transcription_delta, apply_overlay_format_result, clear_transcription_text,
    enter_decision_mode, enter_overlay_formatting, enter_processing_mode, enter_recording_mode,
    get_transcription_text, hide_transcription_overlay, is_transcription_overlay_visible,
    schedule_auto_hide, set_transcription_action_contract, set_transcription_text,
    update_transcription_status,
};
pub use self::state::{TranscriptionActionContractMode, TranscriptionOverlayConfig};
pub use self::window::show_transcription_overlay;
