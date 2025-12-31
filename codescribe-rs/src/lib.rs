//! CodeScribe - Speech-to-text tray app for macOS
//!
//! This is the library interface for CodeScribe components.
//! The main binary is in `main.rs`.

// Allow unexpected cfgs from objc crate's msg_send! macro
#![allow(unexpected_cfgs)]

pub mod audio;
pub mod clipboard;
pub mod config;
pub mod settings;
pub mod voice_chat;

#[cfg(target_os = "macos")]
pub mod ui;

#[cfg(target_os = "macos")]
pub mod voice_chat_ui;

// Re-export commonly used types
pub use audio::{Recorder, RecorderConfig, RecorderDiagnostics};

#[cfg(target_os = "macos")]
pub use ui::{
    focused_element_accepts_text, get_caret_position, get_cursor_position, hide_hold_badge,
    show_hold_badge, show_hold_badge_with_config, HoldBadgeConfig,
};

#[cfg(target_os = "macos")]
pub use voice_chat_ui::{
    append_voice_chat_delta, clear_voice_chat_text, hide_voice_chat_overlay,
    is_voice_chat_overlay_visible, reset_voice_chat_activity, set_voice_chat_text,
    show_voice_chat_overlay, show_voice_chat_overlay_with_config, update_voice_chat_status,
    VoiceChatOverlayConfig,
};
