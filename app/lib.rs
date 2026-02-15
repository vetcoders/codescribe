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
    safe_path, state, status, stream_postprocess, stt, whisper,
};

pub use codescribe_core::{
    get_assistive_prompt_path, get_formatting_prompt_path, reset_to_defaults,
};

// ═══════════════════════════════════════════════════════════
// App/macOS-specific modules
// ═══════════════════════════════════════════════════════════

pub mod os;

#[cfg(target_os = "macos")]
pub mod controller;

#[cfg(target_os = "macos")]
pub mod presentation;

#[cfg(target_os = "macos")]
pub mod ipc;

#[cfg(target_os = "macos")]
pub mod ui;

#[cfg(target_os = "macos")]
pub mod ui_helpers;

#[cfg(target_os = "macos")]
pub mod voice_chat_ui;

#[cfg(target_os = "macos")]
pub mod transcription_overlay;

#[cfg(target_os = "macos")]
pub mod dev;

#[cfg(target_os = "macos")]
pub use ui::{
    BadgeMode, HoldBadgeConfig, focused_element_accepts_text, get_caret_position,
    get_cursor_position, hide_hold_badge, install_basic_edit_menu, set_dock_icon,
    show_badge_for_mode, show_hold_badge, show_hold_badge_with_config,
};

#[cfg(target_os = "macos")]
pub use ui::bootstrap::{
    hide_bootstrap_overlay, hide_settings_window, schedule_bootstrap, schedule_settings_window,
    should_show_bootstrap, should_show_settings_onboarding, show_bootstrap_overlay,
    show_settings_window,
};

#[cfg(target_os = "macos")]
pub use ui::onboarding::{should_show_onboarding, show_onboarding_wizard};

#[cfg(target_os = "macos")]
pub use ui::tray;

#[cfg(target_os = "macos")]
pub use voice_chat_ui::{
    VoiceChatOverlayConfig, add_voice_chat_error_message, add_voice_chat_user_message,
    append_voice_chat_assistant_delta, append_voice_chat_user_delta, clear_voice_chat_text,
    filter_drawer, hide_voice_chat_overlay, is_auto_send_enabled, is_voice_chat_overlay_visible,
    refresh_drawer, reset_voice_chat_activity, send_voice_chat_draft, set_voice_chat_send_callback,
    set_voice_chat_sending, set_voice_chat_text, set_voice_chat_user_text, show_agent_tab,
    show_drawer_tab, show_settings_tab, show_voice_chat_overlay,
    show_voice_chat_overlay_with_config, update_voice_chat_status,
};

#[cfg(target_os = "macos")]
pub use transcription_overlay::{
    TranscriptionOverlayConfig, append_transcription_delta, clear_transcription_text,
    enter_decision_mode, enter_recording_mode, get_transcription_text, hide_transcription_overlay,
    is_transcription_overlay_visible, schedule_auto_hide, set_transcription_text,
    show_transcription_overlay, update_transcription_status,
};

#[cfg(target_os = "macos")]
pub use os::clipboard;
#[cfg(target_os = "macos")]
pub use os::hotkeys;
