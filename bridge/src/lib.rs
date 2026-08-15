//! UniFFI bridge over the LIVING codescribe engine.
//!
//! Strategy (Option B): do NOT re-port the engine. Wrap the real, already-working
//! `codescribe_core` + `codescribe` (provider/tools/config/stt) in a thin UniFFI
//! surface so the new SwiftUI app can drive real streaming agent replies, STT, and
//! config. Mirrors the UniFFI pattern proved in vista-kernel's `qube-ffi`.
//!
//! Layout (W3 cut #0 — split for conflict-free parallel work):
//!   - `agent`     — CodescribeAgent + CsAgentListener (streaming chat)        [live]
//!   - `agent_status` — CodescribeAgentStatus (read-only readiness + MCP status) [W-C1]
//!   - `mcp_admin` — CodescribeMcpAdmin (add/update/remove/test MCP servers)     [W-C4]
//!   - `config`    — CodescribeConfig (settings/prompts/keychain/onboarding)   [W3 #1]
//!   - `recording` — shared controller listener + audio/model settings          [live]
//!   - `threads`   — CodescribeThreads (thread persistence + history)          [W3 #5]
//!
//! Shared cross-slice types (`CsError`, `CsLanguage`) live here so each submodule
//! references one canonical definition.

uniffi::setup_scaffolding!();

/// Streaming agent chat surface (`CodescribeAgent` + listener).
mod agent;
/// Agent delivery callbacks into Swift UI.
mod agent_delivery;
/// Read-only agent readiness and MCP status.
mod agent_status;
/// Settings, prompts, keychain, and onboarding config.
mod config;
/// Global hotkey registration and app-action callbacks.
mod hotkeys;
/// CSK1 license state exposed to the Swift shell.
mod licensing;
/// MCP server admin: add/update/remove/test.
mod mcp_admin;
/// Notes surface bridged for agent tools / UI.
mod notes;
/// Overlay quality records and lexicon commit helpers.
mod quality;
/// Dictation / STT streaming into the Swift app.
mod recording;
/// Thread persistence and history for agent chats.
mod threads;
/// Menu-bar tray status payloads and listener.
mod tray_status;

pub use agent::{CodescribeAgent, CsAgentListener};
pub use agent_delivery::CsAgentDeliveryListener;
pub use hotkeys::CodescribeHotkeys;
pub use hotkeys::CsAppActionListener;
pub use licensing::{CsLicenseState, CsLicenseStatus};
pub use quality::{
    CsLexiconEntry, CsOverlayHighlight, CsOverlayHighlightKind, CsQualityCommitResult,
    CsQualityRecord, commit_overlay_quality_record, lexicon_custom_entries,
    overlay_highlights_enabled, quality_finalize_correction, quality_recent_records,
    quality_teach_span,
};
pub use tray_status::{
    CodescribeTrayStatus, CsTrayStatusKind, CsTrayStatusListener, CsTrayStatusPayload,
    CsTrayStatusTone,
};

/// Error surfaced across the FFI boundary. One enum for every slice:
/// `Agent` (chat/provider), `Config` (settings/keychain/prompt I/O),
/// `Recording` (STT/audio), `License` (CSK1 validation), and `Quality`
/// (overlay quality records).
#[derive(uniffi::Error, Debug)]
pub enum CsError {
    Agent { msg: String },
    Config { msg: String },
    Recording { msg: String },
    License { msg: String },
    Quality { msg: String },
}

impl std::fmt::Display for CsError {
    /// Display the variant message only (no enum tag prefix).
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CsError::Agent { msg }
            | CsError::Config { msg }
            | CsError::Recording { msg }
            | CsError::License { msg }
            | CsError::Quality { msg } => {
                write!(f, "{msg}")
            }
        }
    }
}

impl std::error::Error for CsError {}

impl From<anyhow::Error> for CsError {
    /// Map `anyhow` failures onto the Agent error variant by default.
    fn from(error: anyhow::Error) -> Self {
        CsError::Agent {
            msg: error.to_string(),
        }
    }
}

impl From<std::io::Error> for CsError {
    /// Map I/O failures onto the Config error variant.
    fn from(error: std::io::Error) -> Self {
        CsError::Config {
            msg: error.to_string(),
        }
    }
}

/// Language shared across the config (whisper language setting) and recording
/// (dictation language) surfaces. Maps 1:1 to `codescribe_core::config::Language`.
#[derive(uniffi::Enum, Debug, Clone, Copy, PartialEq, Eq)]
pub enum CsLanguage {
    /// Let Whisper detect the spoken language per recording.
    Auto,
    /// Force Polish decoding (`"pl"`).
    Polish,
    /// Force English decoding (`"en"`).
    English,
}

impl From<codescribe_core::config::Language> for CsLanguage {
    /// Core config language → UniFFI `CsLanguage`.
    fn from(language: codescribe_core::config::Language) -> Self {
        match language {
            codescribe_core::config::Language::Auto => CsLanguage::Auto,
            codescribe_core::config::Language::Polish => CsLanguage::Polish,
            codescribe_core::config::Language::English => CsLanguage::English,
        }
    }
}

impl From<CsLanguage> for codescribe_core::config::Language {
    /// UniFFI `CsLanguage` → core config language.
    fn from(language: CsLanguage) -> Self {
        match language {
            CsLanguage::Auto => codescribe_core::config::Language::Auto,
            CsLanguage::Polish => codescribe_core::config::Language::Polish,
            CsLanguage::English => codescribe_core::config::Language::English,
        }
    }
}

impl CsLanguage {
    /// Two-letter code (`"pl"` / `"en"`) as the core uses it.
    pub fn as_code(&self) -> &'static str {
        match self {
            CsLanguage::Auto => "auto",
            CsLanguage::Polish => "pl",
            CsLanguage::English => "en",
        }
    }
}
