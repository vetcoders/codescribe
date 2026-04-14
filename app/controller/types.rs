//! Controller types and validation
//!
//! Contains type definitions for the recording controller state machine.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

/// A validated audio file path that is guaranteed to be within allowed directories.
///
/// This newtype wrapper ensures at the type level that the path has been validated
/// against path traversal attacks before any file operations are performed.
#[derive(Debug, Clone)]
pub struct ValidatedAudioPath(PathBuf);

impl ValidatedAudioPath {
    /// Create a new ValidatedAudioPath after security validation.
    ///
    /// This prevents path traversal attacks by ensuring the path:
    /// 1. Exists and is a file
    /// 2. Is within an allowed directory (temp dir or ~/.codescribe)
    /// 3. After canonicalization, still resolves to an allowed directory
    ///
    /// Returns Ok(ValidatedAudioPath) if valid, or an error if validation fails.
    pub fn new(path: &Path) -> Result<Self> {
        // Path must exist
        if !path.exists() {
            anyhow::bail!("Audio file does not exist: {:?}", path);
        }

        // Must be a file, not a directory
        if !path.is_file() {
            anyhow::bail!("Audio path is not a file: {:?}", path);
        }

        // Canonicalize to resolve symlinks and get absolute path
        let canonical = path
            .canonicalize()
            .with_context(|| format!("Failed to canonicalize audio path: {:?}", path))?;

        // Define allowed directories
        let temp_dir = std::env::temp_dir();
        let home_codescribe = directories::BaseDirs::new()
            .map(|b| b.home_dir().join(".codescribe"))
            .unwrap_or_else(|| PathBuf::from(".codescribe"));

        // Canonicalize allowed dirs (they might not exist yet)
        let allowed_dirs: Vec<PathBuf> = vec![
            temp_dir.canonicalize().unwrap_or(temp_dir),
            home_codescribe.canonicalize().unwrap_or(home_codescribe),
        ];

        // Check if canonical path starts with any allowed directory
        let is_allowed = allowed_dirs
            .iter()
            .any(|allowed| canonical.starts_with(allowed));

        if !is_allowed {
            anyhow::bail!(
                "Audio path {:?} is outside allowed directories. Canonical: {:?}",
                path,
                canonical
            );
        }

        Ok(Self(canonical))
    }

    /// Get a reference to the validated path.
    pub fn as_path(&self) -> &Path {
        &self.0
    }
}

/// Application state enum
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    /// Waiting for user input
    Idle,
    /// Recording in hold-to-talk mode
    RecHold,
    /// Recording in toggle mode
    RecToggle,
    /// Processing transcription and formatting
    Busy,
    /// Full-duplex conversation mode (Moshi)
    ///
    /// In this mode, the app simultaneously:
    /// - Records audio from microphone
    /// - Processes through VAD + Moshi LM
    /// - Plays AI response through speaker
    /// - Supports interruption (user can speak while AI responds)
    Conversation,
}

impl std::fmt::Display for State {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            State::Idle => write!(f, "IDLE"),
            State::RecHold => write!(f, "REC_HOLD"),
            State::RecToggle => write!(f, "REC_TOGGLE"),
            State::Busy => write!(f, "BUSY"),
            State::Conversation => write!(f, "CONVERSATION"),
        }
    }
}

impl State {
    pub fn to_ipc_str(self) -> &'static str {
        match self {
            State::Idle => "idle",
            State::RecHold => "rec_hold",
            State::RecToggle => "rec_toggle",
            State::Busy => "busy",
            State::Conversation => "conversation",
        }
    }
}

/// Hotkey event types
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HotkeyType {
    Hold,
    Toggle,
    /// Full-duplex conversation mode (Ctrl+Option)
    Conversation,
}

/// Hotkey action types
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HotkeyAction {
    Down,
    Up,
    Press,
}

/// Complete hotkey event with metadata
#[derive(Debug, Clone)]
pub struct HotkeyInput {
    pub key_type: HotkeyType,
    pub action: HotkeyAction,
    pub assistive: bool,
    pub hold_mode: crate::os::hotkeys::HoldMode,
    pub force_raw: bool,
    pub force_ai: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RecordingTranscriptSource {
    LocalFinalPass,
    CloudPrimary,
    CloudFallback,
    Streaming,
    StreamingFallback,
}

impl RecordingTranscriptSource {
    pub fn label(self) -> &'static str {
        match self {
            Self::LocalFinalPass => "Final-pass local",
            Self::CloudPrimary => "Cloud primary",
            Self::CloudFallback => "Cloud fallback",
            Self::Streaming => "Streaming preview",
            Self::StreamingFallback => "Streaming fallback",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RecordingFallbackClass {
    Acceptable,
    Degraded,
    Unsafe,
}

impl RecordingFallbackClass {
    pub fn label(self) -> &'static str {
        match self {
            Self::Acceptable => "acceptable fallback",
            Self::Degraded => "degraded fallback",
            Self::Unsafe => "unsafe fallback",
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct RecordingTruthMetadata {
    pub source: Option<RecordingTranscriptSource>,
    pub engine: Option<String>,
    pub mode: Option<String>,
    pub fallback_class: Option<RecordingFallbackClass>,
    pub fallback_used: bool,
    pub vad_speech_pct: Option<f32>,
    pub no_speech_reason: Option<String>,
    pub avg_logprob: Option<f32>,
    #[serde(default)]
    pub confidence_flags: Vec<String>,
    pub commit_trigger: Option<String>,
    pub display_status: Option<String>,
}

pub fn truth_sidecar_path(path: &Path) -> PathBuf {
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .map(|name| format!("{name}.truth.json"))
        .unwrap_or_else(|| "artifact.truth.json".to_string());
    path.with_file_name(file_name)
}

pub fn write_truth_sidecar(path: &Path, metadata: &RecordingTruthMetadata) -> Result<PathBuf> {
    let sidecar_path = truth_sidecar_path(path);
    let payload =
        serde_json::to_vec_pretty(metadata).context("Failed to serialize truth sidecar")?;
    fs::write(&sidecar_path, payload)
        .with_context(|| format!("Failed to write truth sidecar {}", sidecar_path.display()))?;
    Ok(sidecar_path)
}

#[cfg(test)]
pub fn read_truth_sidecar(path: &Path) -> Result<RecordingTruthMetadata> {
    let sidecar_path = truth_sidecar_path(path);
    let payload = codescribe_core::util::safe_path::safe_read_to_string(&sidecar_path)
        .with_context(|| format!("Failed to read truth sidecar {}", sidecar_path.display()))?;
    serde_json::from_str(&payload)
        .with_context(|| format!("Failed to parse truth sidecar {}", sidecar_path.display()))
}

/// Parameters for the transcript text pipeline.
///
/// Groups all inputs for `process_transcript_text_pipeline` to avoid
/// a 16-argument function signature.
pub struct TranscriptPipelineParams {
    pub raw_text: String,
    pub recording_timestamp: chrono::DateTime<chrono::Local>,
    pub assistive: bool,
    pub hold_mode: crate::os::hotkeys::HoldMode,
    pub force_raw: bool,
    pub force_ai: bool,
    pub config: crate::config::Config,
    pub language_opt: Option<String>,
    pub raw_save_enabled: bool,
    pub audio_path: Option<ValidatedAudioPath>,
    pub cloud_text_opt: Option<String>,
    pub cloud_handle: Option<tokio::task::JoinHandle<anyhow::Result<String>>>,
    pub transcript_source: Option<RecordingTranscriptSource>,
    pub truth_fallback_class: Option<RecordingFallbackClass>,
    pub truth_no_speech_reason: Option<String>,
    pub truth_speech_pct: Option<f32>,
    pub truth_avg_logprob: Option<f32>,
    pub truth_confidence_flags: Vec<String>,
    pub truth_commit_trigger: Option<String>,
    pub truth_display_status: String,
    pub append_mode: bool,
    /// True when processing happens while an active stream is still running
    /// (e.g., toggle-mode utterance callback). In this mode, prefer delta-only
    /// updates and avoid full-text rewrites in overlays.
    pub live_stream_session: bool,
    pub user_needs_separator: bool,
    pub assistant_needs_separator: bool,
    /// When true, skip writing to the user bubble in the commit path.
    /// Used by event pipeline where Preview already streams into the bubble.
    pub skip_user_bubble: bool,
}

/// Result metadata for transcript post-processing.
#[derive(Debug, Clone, Default)]
pub struct TranscriptProcessOutcome {
    /// Why manual commit/decision mode should be shown (if required).
    pub commit_trigger: Option<String>,
    pub final_status: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn truth_sidecar_roundtrip_preserves_metadata() {
        let temp_dir = tempfile::tempdir().expect("temp dir");
        let transcript_path = temp_dir.path().join("sample_raw.txt");
        fs::write(&transcript_path, "hello").expect("write transcript");

        let metadata = RecordingTruthMetadata {
            source: Some(RecordingTranscriptSource::StreamingFallback),
            engine: Some("streaming_whisper".to_string()),
            mode: Some("toggle".to_string()),
            fallback_class: Some(RecordingFallbackClass::Degraded),
            fallback_used: true,
            vad_speech_pct: Some(42.0),
            no_speech_reason: None,
            avg_logprob: Some(-0.25),
            confidence_flags: vec!["cloud_primary_missing".to_string()],
            commit_trigger: Some("cloud_failed_fallback".to_string()),
            display_status: Some("Streaming fallback".to_string()),
        };

        let sidecar_path = write_truth_sidecar(&transcript_path, &metadata).expect("write sidecar");
        assert_eq!(sidecar_path, truth_sidecar_path(&transcript_path));

        let restored = read_truth_sidecar(&transcript_path).expect("read sidecar");
        assert_eq!(restored, metadata);
    }
}
