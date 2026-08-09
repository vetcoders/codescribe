//! Codescribe - native macOS tray dictation app with runtime Whisper lookup.
//!
//! This crate re-exports the core functionality from `codescribe_core`
//! and provides the macOS-specific UI, tray, and hotkey layers.

// ═══════════════════════════════════════════════════════════
// Core re-exports
// ═══════════════════════════════════════════════════════════

pub use codescribe_core::{
    Recorder, RecorderConfig, ai_formatting, audio, client, config, qube_daemon, qube_report,
    safe_path, state, status, stream_postprocess, stt, whisper,
};

pub use codescribe_core::{
    get_assistive_prompt_path, get_formatting_prompt_path, reset_to_defaults,
};

// ═══════════════════════════════════════════════════════════
// App/macOS-specific modules
// ═══════════════════════════════════════════════════════════

/// Assistive agent runtime: providers, tool registry, and the run monitor service.
pub mod agent;

/// Process-global broadcast carrying voice-assistive reply events to the UI.
pub mod agent_delivery;
/// Tracing/log initialization shared by the app and the UniFFI bridge.
pub mod logging;
/// macOS platform layer: hotkeys, clipboard, onboarding, permissions.
pub mod os;
#[cfg(test)]
pub(crate) mod test_env;

/// Recording lifecycle owner: start/stop paths, final pass, delivery.
#[cfg(target_os = "macos")]
pub mod controller;

/// Native presentation surfaces (overlay, tray, status rendering).
#[cfg(target_os = "macos")]
pub mod presentation;

#[cfg(target_os = "macos")]
pub use os::onboarding::{
    load_onboarding_progress, mark_onboarding_done, save_onboarding_progress,
    should_show_onboarding,
};

#[cfg(target_os = "macos")]
pub use os::clipboard;
#[cfg(target_os = "macos")]
pub use os::hotkeys;
