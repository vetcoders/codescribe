//! CodeScribe - Speech-to-text with embedded Whisper model
//!
//! ## Quick Start
//!
//! ```ignore
//! // Transcribe with embedded model (zero config!)
//! codescribe::whisper::init()?;
//! let text = codescribe::whisper::transcribe(&samples, 16000, Some("pl"))?;
//! ```
//!
//! ## Architecture
//!
//! - **whisper** - Embedded Whisper model (~900MB in binary), zero I/O
//! - **audio** - Recording and audio loading
//! - **config** - User configuration
//! - **ai_formatting** - Post-processing with LLMs
//!
//! Created by M&K (c)2026 VetCoders

#![allow(unexpected_cfgs)]

// ═══════════════════════════════════════════════════════════
// Core modules
// ═══════════════════════════════════════════════════════════

pub mod ai_formatting;
pub mod audio;
pub mod clipboard;
pub mod config;
pub mod permissions;
pub mod state;
pub mod voice_chat;
pub mod whisper;

// ═══════════════════════════════════════════════════════════
// macOS-specific modules
// ═══════════════════════════════════════════════════════════

#[cfg(target_os = "macos")]
pub mod hotkeys;

#[cfg(target_os = "macos")]
pub mod ui;

#[cfg(target_os = "macos")]
pub mod voice_chat_ui;

// ═══════════════════════════════════════════════════════════
// Public API - Whisper (main interface)
// ═══════════════════════════════════════════════════════════

/// Initialize and transcribe with embedded model
pub mod stt {
    pub use crate::whisper::{
        init, is_initialized, transcribe, transcribe_file, transcribe_streaming,
        detect_language, get_model_path,
    };
    pub use crate::whisper::embedded::{is_embedded_available, get_embedded_data, EmbeddedModel};
}

// ═══════════════════════════════════════════════════════════
// Public API - Audio
// ═══════════════════════════════════════════════════════════

pub use audio::{Recorder, RecorderConfig, RecorderDiagnostics};

// ═══════════════════════════════════════════════════════════
// Public API - AI & Context
// ═══════════════════════════════════════════════════════════

pub use config::{get_assistive_prompt_path, get_formatting_prompt_path, reset_to_defaults};
pub use state::has_active_conversation;

#[cfg(target_os = "macos")]
pub use ui::{
    BadgeMode, HoldBadgeConfig, focused_element_accepts_text, get_caret_position,
    get_cursor_position, hide_hold_badge, set_dock_icon, show_badge_for_mode, show_hold_badge,
    show_hold_badge_with_config,
};

#[cfg(target_os = "macos")]
pub use voice_chat_ui::{
    VoiceChatOverlayConfig, append_voice_chat_delta, clear_voice_chat_text,
    hide_voice_chat_overlay, is_voice_chat_overlay_visible, reset_voice_chat_activity,
    set_voice_chat_text, show_voice_chat_overlay, show_voice_chat_overlay_with_config,
    update_voice_chat_status,
};
