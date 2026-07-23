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
//!   - `recording` — CodescribeDictation + CsTranscriptionListener (STT)       [W3 #3]
//!   - `threads`   — CodescribeThreads (thread persistence + history)          [W3 #5]
//!
//! Shared cross-slice types (`CsError`, `CsLanguage`) live here so each submodule
//! references one canonical definition.

uniffi::setup_scaffolding!();

mod agent;
mod agent_delivery;
mod agent_status;
mod config;
mod hotkeys;
mod mcp_admin;
mod notes;
mod quality;
mod recording;
mod threads;
mod tray_status;

pub use agent::{CodescribeAgent, CsAgentListener};
pub use agent_delivery::CsAgentDeliveryListener;
pub use hotkeys::CodescribeHotkeys;
pub use hotkeys::CsAppActionListener;
pub use quality::{
    CsLexiconEntry, CsQualityCommitResult, CsQualityRecord, commit_overlay_quality_record,
    lexicon_custom_entries, quality_finalize_correction, quality_recent_records,
};
pub use tray_status::{
    CodescribeTrayStatus, CsTrayStatusKind, CsTrayStatusListener, CsTrayStatusPayload,
    CsTrayStatusTone,
};

/// Error surfaced across the FFI boundary. One enum for every slice:
/// `Agent` (chat/provider), `Config` (settings/keychain/prompt I/O),
/// `Recording` (STT/audio).
#[derive(uniffi::Error, Debug)]
pub enum CsError {
    Agent { msg: String },
    Config { msg: String },
    Recording { msg: String },
    Quality { msg: String },
}

impl std::fmt::Display for CsError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CsError::Agent { msg }
            | CsError::Config { msg }
            | CsError::Recording { msg }
            | CsError::Quality { msg } => {
                write!(f, "{msg}")
            }
        }
    }
}

impl std::error::Error for CsError {}

impl From<anyhow::Error> for CsError {
    fn from(error: anyhow::Error) -> Self {
        CsError::Agent {
            msg: error.to_string(),
        }
    }
}

impl From<std::io::Error> for CsError {
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
    Auto,
    Polish,
    English,
}

impl From<codescribe_core::config::Language> for CsLanguage {
    fn from(language: codescribe_core::config::Language) -> Self {
        match language {
            codescribe_core::config::Language::Auto => CsLanguage::Auto,
            codescribe_core::config::Language::Polish => CsLanguage::Polish,
            codescribe_core::config::Language::English => CsLanguage::English,
        }
    }
}

impl From<CsLanguage> for codescribe_core::config::Language {
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
