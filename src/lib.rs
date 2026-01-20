//! CodeScribe - Speech-to-text with embedded Whisper model
//!
//! This crate re-exports the core functionality from `codescribe_core`
//! and provides the macOS-specific UI, tray, and hotkey layers.
//!
//! Created by M&K (c)2026 VetCoders

// ═══════════════════════════════════════════════════════════
// Core re-exports
// ═══════════════════════════════════════════════════════════

pub use codescribe_core::{
    Recorder, RecorderConfig, ai_formatting, audio, client, config, quality_loop, quality_report,
    safe_path, state, status, stream_postprocess, stt, voice_chat, whisper,
};

pub use codescribe_core::{
    get_assistive_prompt_path, get_formatting_prompt_path, reset_to_defaults,
};

// ═══════════════════════════════════════════════════════════
// App/macOS-specific modules
// ═══════════════════════════════════════════════════════════

pub mod clipboard;
pub mod permissions;

#[cfg(target_os = "macos")]
pub mod hotkeys;

#[cfg(target_os = "macos")]
pub mod controller;

#[cfg(target_os = "macos")]
pub mod ipc;

#[cfg(target_os = "macos")]
pub mod ui;

#[cfg(target_os = "macos")]
pub mod voice_chat_ui;

#[cfg(target_os = "macos")]
pub mod tray;

#[cfg(target_os = "macos")]
pub use ui::{
    BadgeMode, HoldBadgeConfig, focused_element_accepts_text, get_caret_position,
    get_cursor_position, hide_hold_badge, set_dock_icon, show_badge_for_mode, show_hold_badge,
    show_hold_badge_with_config,
};

#[cfg(target_os = "macos")]
pub use voice_chat_ui::{
    VoiceChatOverlayConfig, add_voice_chat_error_message, add_voice_chat_user_message,
    append_voice_chat_assistant_delta, append_voice_chat_delta, clear_voice_chat_text,
    hide_voice_chat_overlay, is_auto_send_enabled, is_voice_chat_overlay_visible,
    reset_voice_chat_activity, send_voice_chat_draft, set_voice_chat_draft_text,
    set_voice_chat_send_callback, set_voice_chat_sending, set_voice_chat_text,
    show_voice_chat_overlay, show_voice_chat_overlay_with_config, update_voice_chat_status,
};
