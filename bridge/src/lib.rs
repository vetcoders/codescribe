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

/// Seal the embedding application's state and credential identity before any
/// config, agent, account or recording handle is constructed.
#[uniffi::export]
pub fn configure_embedded_runtime(
    data_directory: String,
    keychain_service: String,
) -> Result<(), CsError> {
    let host = codescribe_core::config::runtime_host::RuntimeHost::new(
        data_directory.into(),
        keychain_service,
    )?;
    codescribe_core::config::runtime_host::configure(host)?;
    Ok(())
}

/// Streaming agent chat surface (`CodescribeAgent` + listener).
mod agent;
/// Agent delivery callbacks into Swift UI.
mod agent_delivery;
/// Read-only agent readiness and MCP status.
mod agent_status;
/// Process-owned async runtime and lifecycle evidence.
mod application_runtime;
/// Settings, prompts, keychain, and onboarding config.
mod config;
/// Live buffer tools supplied by an embedding document editor.
mod document_agent;
/// Global hotkey registration and app-action callbacks.
mod hotkeys;
/// CSK1 license state exposed to the Swift shell.
mod licensing;
/// MCP server admin: add/update/remove/test.
mod mcp_admin;
/// Notes surface bridged for agent tools / UI.
mod notes;
/// Host on-device formatting (Apple FoundationModels) registration (W6).
mod on_device_format;
/// Overlay quality records and lexicon commit helpers.
mod quality;
/// Dictation / STT streaming into the Swift app.
mod recording;
/// Vendor speech synthesis and cancellation.
mod speech;
/// Thread persistence and history for agent chats.
mod threads;
/// Menu-bar tray status payloads and listener.
mod tray_status;
/// Private, app-owned vocabulary replay.
mod vocabulary_ab;
/// Workspace discovery and explicit document opening for an embedded session.
mod workspace_agent;

pub use agent::{CodescribeAgent, CsAgentListener};
pub use agent_delivery::CsAgentDeliveryListener;
pub use application_runtime::CsApplicationRuntimeSnapshot;
pub use hotkeys::CodescribeHotkeys;
pub use hotkeys::CsAppActionListener;
pub use licensing::{CsLicenseState, CsLicenseStatus};
pub use on_device_format::{CsOnDeviceFormatOutcome, CsOnDeviceFormatter};
pub use quality::{
    CsDiffSpan, CsDiffTier, CsLexiconEntry, CsQualityCommitResult, CsQualityListing,
    CsQualityRecord, CsRuleCandidate, commit_overlay_quality_record, lexicon_custom_entries,
    quality_diff_spans, quality_finalize_correction, quality_recent_listing,
    quality_rule_candidates, quality_teach_span,
};
#[cfg(unix)]
pub use recording::CsAudioReadLease;
pub use recording::{CsCaptureHandle, CsConditionalStop, CsTranscriptDelivery};
pub use speech::{CsSpeechResult, speak_text, speech_availability, stop_speaking};
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
    Runtime { msg: String },
}

impl std::fmt::Display for CsError {
    /// Display the variant message only (no enum tag prefix).
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CsError::Agent { msg }
            | CsError::Config { msg }
            | CsError::Recording { msg }
            | CsError::License { msg }
            | CsError::Quality { msg }
            | CsError::Runtime { msg } => {
                write!(f, "{msg}")
            }
        }
    }
}

impl std::error::Error for CsError {}

/// Passive conversation observation uses the existing runtime bus path owner.
#[uniffi::export]
pub fn agent_conversation_bus_path() -> String {
    codescribe::presentation::transcript_bus::transcript_bus_path()
        .to_string_lossy()
        .into_owned()
}

/// Validate and recover managed journal linkage before the first take. The
/// potentially expensive archive verification never runs on the UI thread.
#[uniffi::export]
pub async fn prepare_transcript_storage() -> Result<bool, CsError> {
    application_runtime::run(async {
        tokio::task::spawn_blocking(|| {
            let path = codescribe::presentation::transcript_bus::transcript_bus_path();
            prepare_transcript_storage_at(&path, &codescribe_core::stt::active_names::bridge_home())
        })
        .await
        .map_err(|error| CsError::Recording {
            msg: error.to_string(),
        })?
        .map_err(|error| CsError::Recording {
            msg: format!("Transcript storage unavailable: {error}"),
        })
    })
    .await?
}

fn prepare_transcript_storage_at(
    path: &std::path::Path,
    bridge_home: &std::path::Path,
) -> std::io::Result<bool> {
    use codescribe::presentation::transcript_bus_maintenance::generation;
    use std::{fs, io};

    let mut receipt = path.as_os_str().to_os_string();
    receipt.push(".generations.json");
    for selected in [path, std::path::Path::new(&receipt)] {
        match fs::symlink_metadata(selected) {
            Ok(metadata) if metadata.is_file() => {}
            Ok(_) => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!(
                        "Transcript storage is not a regular file: {}",
                        selected.display()
                    ),
                ));
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
    }
    let mut paths = std::collections::BTreeSet::from([path.to_path_buf()]);
    let buses = bridge_home.join("buses");
    let entries = match fs::symlink_metadata(&buses) {
        Ok(metadata) if metadata.is_dir() => Some(fs::read_dir(&buses)?),
        Ok(_) => {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("Channel storage is not a directory: {}", buses.display()),
            ));
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => None,
        Err(error) => return Err(error),
    };
    if let Some(entries) = entries {
        for entry in entries {
            let entry = entry?;
            let name = entry.file_name();
            let Some(name) = name.to_str() else {
                continue;
            };
            if !name.starts_with("channel-") || !name.ends_with(".jsonl") {
                continue;
            }
            let receipt = entry
                .path()
                .with_file_name(format!("{name}.generations.json"));
            match fs::symlink_metadata(&receipt) {
                Ok(metadata) if metadata.is_file() && entry.file_type()?.is_file() => {
                    paths.insert(entry.path());
                }
                Ok(_) => {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        format!(
                            "Managed channel storage is not a regular file: {}",
                            entry.path().display()
                        ),
                    ));
                }
                Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                Err(error) => return Err(error),
            }
        }
    }
    let mut recovered = false;
    for path in paths {
        recovered |= generation::recover_device_number(&path).map_err(|error| {
            io::Error::new(error.kind(), format!("{}: {error}", path.display()))
        })?;
    }
    Ok(recovered)
}

/// Start the one process-owned async runtime. Idempotent while running; once
/// shut down it cannot be restarted in the same process.
#[uniffi::export]
pub fn start_application_runtime() -> Result<CsApplicationRuntimeSnapshot, CsError> {
    start_application_runtime_with_compaction(|| {
        let path = codescribe::presentation::transcript_bus::transcript_bus_path();
        if let Err(error) =
            codescribe::presentation::transcript_bus_maintenance::compact_bus_if_enabled(
                &path, "startup",
            )
        {
            tracing::warn!(%error, "startup bus compaction unavailable");
        }
    })
}

/// Inject the startup compaction work for the thread-ordering test. The app
/// uses the production closure above; both paths use the same spawn boundary.
#[doc(hidden)]
pub fn start_application_runtime_with_compaction(
    compaction: impl FnOnce() + Send + 'static,
) -> Result<CsApplicationRuntimeSnapshot, CsError> {
    let snapshot = application_runtime::start()?;
    if let Err(error) = std::thread::Builder::new()
        .name("codescribe-bus-compaction".to_string())
        .spawn(compaction)
    {
        tracing::warn!(%error, "startup bus compaction thread unavailable");
    }
    Ok(snapshot)
}

/// Content-free lifecycle snapshot used by diagnostics and delivery probes.
#[uniffi::export]
pub fn application_runtime_snapshot() -> Result<CsApplicationRuntimeSnapshot, CsError> {
    application_runtime::snapshot()
}

/// Stop controller/account activity first, then tear down every runtime worker.
#[uniffi::export]
pub fn shutdown_application_runtime() -> Result<CsApplicationRuntimeSnapshot, CsError> {
    config::cancel_pending_account_login_for_shutdown();
    speech::stop_speaking();
    hotkeys::shutdown_application_controller()?;
    application_runtime::shutdown()
}

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

impl From<codescribe_core::config::SettingsSnapshotValidationError> for CsError {
    fn from(error: codescribe_core::config::SettingsSnapshotValidationError) -> Self {
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

#[cfg(test)]
mod startup_tests {
    use std::sync::mpsc;
    use std::time::Duration;

    #[test]
    fn storage_preparation_skips_unmanaged_files_and_reports_a_broken_managed_channel() {
        let root = tempfile::tempdir().unwrap();
        let bus = root.path().join("bus.jsonl");
        let home = root.path().join("agent-bridge");
        std::fs::create_dir_all(home.join("buses")).unwrap();
        let channel = home.join("buses/channel-3.jsonl");
        std::fs::write(&channel, b"untouched unmanaged data\n").unwrap();
        assert!(!super::prepare_transcript_storage_at(&bus, &home).unwrap());
        assert!(!bus.exists());
        let receipt = channel.with_file_name("channel-3.jsonl.generations.json");
        std::fs::write(&receipt, b"{broken").unwrap();
        let error = super::prepare_transcript_storage_at(&bus, &home).unwrap_err();
        assert!(error.to_string().contains("channel-3.jsonl"));
        assert_eq!(
            std::fs::read(&channel).unwrap(),
            b"untouched unmanaged data\n"
        );
        assert_eq!(std::fs::read(&receipt).unwrap(), b"{broken");
    }

    #[test]
    #[cfg(unix)]
    fn storage_preparation_refuses_symlinked_managed_channels_receipts_and_directory() {
        use std::os::unix::fs::symlink;
        for mode in [
            "directory",
            "channel",
            "receipt",
            "main_bus",
            "main_receipt",
        ] {
            let root = tempfile::tempdir().unwrap();
            let bus = root.path().join("bus.jsonl");
            let home = root.path().join("agent-bridge");
            let outside = root.path().join("outside");
            std::fs::create_dir_all(&home).unwrap();
            std::fs::create_dir_all(&outside).unwrap();
            let sentinel = outside.join("sentinel");
            std::fs::write(&sentinel, b"preserve").unwrap();
            if mode == "main_bus" {
                symlink(&sentinel, &bus).unwrap();
            } else if mode == "main_receipt" {
                symlink(&sentinel, bus.with_file_name("bus.jsonl.generations.json")).unwrap();
            } else if mode == "directory" {
                symlink(&outside, home.join("buses")).unwrap();
            } else {
                std::fs::create_dir_all(home.join("buses")).unwrap();
                let channel = home.join("buses/channel-3.jsonl");
                let receipt = home.join("buses/channel-3.jsonl.generations.json");
                if mode == "channel" {
                    symlink(&sentinel, &channel).unwrap();
                    std::fs::write(&receipt, b"{}").unwrap();
                } else {
                    std::fs::write(&channel, b"preserve channel").unwrap();
                    symlink(&sentinel, &receipt).unwrap();
                }
            }
            assert!(
                super::prepare_transcript_storage_at(&bus, &home).is_err(),
                "{mode}"
            );
            assert_eq!(std::fs::read(&sentinel).unwrap(), b"preserve");
        }
    }

    #[test]
    fn startup_returns_while_compaction_thread_is_held_at_entry() {
        let (entered_tx, entered_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let (finished_tx, finished_rx) = mpsc::channel();
        let snapshot = super::start_application_runtime_with_compaction(move || {
            entered_tx.send(()).unwrap();
            release_rx.recv().unwrap();
            finished_tx.send(()).unwrap();
        })
        .unwrap();

        assert_eq!(snapshot.state, "running");
        entered_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        assert!(
            matches!(finished_rx.try_recv(), Err(mpsc::TryRecvError::Empty)),
            "compaction must still be held after startup returned"
        );
        release_tx.send(()).unwrap();
        finished_rx.recv_timeout(Duration::from_secs(5)).unwrap();
    }
}
