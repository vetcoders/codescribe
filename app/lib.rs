//! CodeScribe - native macOS tray dictation app with runtime Whisper lookup.
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

pub mod agent;
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
pub mod dev;

#[cfg(target_os = "macos")]
pub use ui::{
    BadgeMode, HoldBadgeConfig, apply_dock_icon_visibility, focused_element_accepts_text,
    get_caret_position, get_cursor_position, hide_hold_badge, install_basic_edit_menu,
    set_dock_icon, show_badge_for_mode, show_hold_badge, show_hold_badge_with_config,
};

#[cfg(target_os = "macos")]
pub use ui::onboarding::{should_show_onboarding, show_onboarding_wizard};

#[cfg(target_os = "macos")]
pub use ui::tray;

#[cfg(target_os = "macos")]
pub use os::clipboard;
#[cfg(target_os = "macos")]
pub use os::hotkeys;
