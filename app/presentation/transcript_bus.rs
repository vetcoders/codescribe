//! Durable clean transcript events for operator and control-plane consumers.
//!
//! The bus is an observer of the committed [`PresentationEmitter`] reducer. It
//! never opens audio, re-transcribes a file, or reconstructs text from UI
//! deltas. One append-only JSON object is flushed per state transition.

use std::fs::{File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use chrono::{SecondsFormat, Utc};
use codescribe_core::pipeline::contracts::TranscriptSegment;
use serde::{Deserialize, Serialize};

/// Explicit path override for the clean transcript bus.
pub const TRANSCRIPT_BUS_PATH_ENV: &str = "CODESCRIBE_TRANSCRIPT_BUS_PATH";
/// Stable filename under the configured state/data root.
pub const TRANSCRIPT_BUS_FILENAME: &str = "transcript-events.jsonl";

/// Product mode attached to every committed transcript event.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TranscriptMode {
    /// Plain dictation or formatting; the downstream action is paste/format.
    Dictation,
    /// Right Option / composer Agent voice input; the downstream action is send.
    Agent,
    /// Hold-based Chat/Selection assistance; downstream action is Agent delivery.
    Assistive,
}

/// Immutable identity supplied by the controller before capture starts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TranscriptSession {
    pub session_id: String,
    pub mode: TranscriptMode,
}

/// One utterance after it has entered the authoritative committed reducer.
#[derive(Debug, Clone, PartialEq)]
pub struct CommittedTranscript {
    pub utterance_id: u64,
    pub text: String,
    pub start_seconds: f32,
    pub end_seconds: f32,
    pub segments: Vec<TranscriptSegment>,
}

/// Append-only public event contract. `text` is always clean reducer truth;
/// unfiltered engine `raw_text` never crosses this boundary.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CleanTranscriptEvent {
    pub schema: String,
    pub sequence: u64,
    pub session_id: String,
    pub mode: TranscriptMode,
    pub utterance_id: Option<u64>,
    pub emitted_at: String,
    pub status: String,
    pub sample_rate_hz: Option<u32>,
    pub sample_start: Option<u64>,
    pub sample_end: Option<u64>,
    pub audio_start_seconds: Option<f32>,
    pub audio_end_seconds: Option<f32>,
    pub text: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub segments: Vec<TranscriptSegment>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pipeline_session_id: Option<String>,
}

/// Synchronous low-frequency writer. Commits occur on the STT worker, not the
/// CoreAudio callback, and each line is flushed so a live tailer sees it before
/// the next utterance or process exit.
pub struct TranscriptBus {
    session: TranscriptSession,
    path: PathBuf,
    writer: Mutex<TranscriptBusWriter>,
    sample_rate_override: Option<u32>,
}

/// One lock owns lifecycle and bytes together. This makes the sequence stored
/// on disk authoritative even when engine-close and a late reducer callback
/// arrive from different threads.
struct TranscriptBusWriter {
    file: File,
    sequence: u64,
    started: bool,
    finalized: bool,
}

impl TranscriptBus {
    /// Resolve the production path and open the session bus. Failure disables
    /// only observability; it must never stop microphone capture or delivery.
    pub fn open(session: TranscriptSession) -> Option<Self> {
        let path = transcript_bus_path();
        match Self::open_at(session, path, None) {
            Ok(bus) => Some(bus),
            Err(error) => {
                tracing::warn!(%error, "clean transcript bus unavailable");
                None
            }
        }
    }

    /// Open an explicit path. Kept public for deterministic pipeline tests and
    /// embedders that already own an XDG/project state root.
    pub fn open_at(
        session: TranscriptSession,
        path: PathBuf,
        sample_rate_override: Option<u32>,
    ) -> io::Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        let mut options = OpenOptions::new();
        options.create(true).append(true).write(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let file = options.open(&path)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            file.set_permissions(std::fs::Permissions::from_mode(0o600))?;
        }

        let bus = Self {
            session,
            path,
            writer: Mutex::new(TranscriptBusWriter {
                file,
                sequence: 0,
                started: false,
                finalized: false,
            }),
            sample_rate_override,
        };
        Ok(bus)
    }

    /// Publish the recording start exactly once. Controllers call this only
    /// after audio starts; commit/final observers also call it defensively so
    /// the first visible transcript event can never precede its session start.
    pub fn publish_started(&self) {
        let mut writer = self
            .writer
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        match self.ensure_started_locked(&mut writer) {
            Ok(true) => {
                tracing::info!(path = %self.path.display(), session_id = %self.session.session_id, mode = ?self.session.mode, "clean transcript bus session started");
            }
            Ok(false) => {}
            Err(error) => self.log_write_error(error),
        }
    }

    /// Publish a new committed slot or a later bounded revision of that slot.
    pub fn publish_utterance(&self, status: &'static str, utterance: CommittedTranscript) {
        let sample_rate = self.sample_rate();
        let event = CleanTranscriptEvent {
            schema: "codescribe.transcript.v1".to_string(),
            sequence: 0,
            session_id: String::new(),
            mode: self.session.mode,
            utterance_id: Some(utterance.utterance_id),
            emitted_at: String::new(),
            status: status.to_string(),
            sample_rate_hz: sample_rate,
            sample_start: sample_rate.map(|rate| seconds_to_sample(utterance.start_seconds, rate)),
            sample_end: sample_rate.map(|rate| seconds_to_sample(utterance.end_seconds, rate)),
            audio_start_seconds: Some(utterance.start_seconds),
            audio_end_seconds: Some(utterance.end_seconds),
            text: utterance.text,
            segments: utterance.segments,
            pipeline_session_id: None,
        };

        let mut writer = self
            .writer
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if writer.finalized {
            tracing::warn!(session_id = %self.session.session_id, %status, "clean transcript event ignored after session finalization");
            return;
        }
        if let Err(error) = self
            .ensure_started_locked(&mut writer)
            .and_then(|_| self.write_event_locked(&mut writer, event))
        {
            self.log_write_error(error);
        }
    }

    /// Publish the immutable session canvas at the engine close boundary.
    pub fn publish_final(&self, text: String, pipeline_session_id: Option<String>) {
        let mut writer = self
            .writer
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if writer.finalized {
            return;
        }
        let event = CleanTranscriptEvent {
            schema: "codescribe.transcript.v1".to_string(),
            sequence: 0,
            session_id: String::new(),
            mode: self.session.mode,
            utterance_id: None,
            emitted_at: String::new(),
            status: "session_finalized".to_string(),
            sample_rate_hz: self.sample_rate(),
            sample_start: None,
            sample_end: None,
            audio_start_seconds: None,
            audio_end_seconds: None,
            text,
            segments: Vec::new(),
            pipeline_session_id,
        };
        match self
            .ensure_started_locked(&mut writer)
            .and_then(|_| self.write_event_locked(&mut writer, event))
        {
            Ok(()) => writer.finalized = true,
            Err(error) => self.log_write_error(error),
        }
    }

    /// The resolved path consumed by an external NDJSON tailer.
    pub fn path(&self) -> &Path {
        &self.path
    }

    fn sample_rate(&self) -> Option<u32> {
        self.sample_rate_override.or_else(|| {
            codescribe_core::audio::capture_receipt::last_open_capture_path()
                .map(|capture| capture.sample_rate)
                .filter(|rate| *rate > 0)
        })
    }

    fn ensure_started_locked(&self, writer: &mut TranscriptBusWriter) -> io::Result<bool> {
        if writer.started {
            return Ok(false);
        }
        self.write_event_locked(
            writer,
            CleanTranscriptEvent {
                schema: "codescribe.transcript.v1".to_string(),
                sequence: 0,
                session_id: String::new(),
                mode: self.session.mode,
                utterance_id: None,
                emitted_at: String::new(),
                status: "session_started".to_string(),
                sample_rate_hz: None,
                sample_start: None,
                sample_end: None,
                audio_start_seconds: None,
                audio_end_seconds: None,
                text: String::new(),
                segments: Vec::new(),
                pipeline_session_id: None,
            },
        )?;
        writer.started = true;
        Ok(true)
    }

    fn write_event_locked(
        &self,
        writer: &mut TranscriptBusWriter,
        mut event: CleanTranscriptEvent,
    ) -> io::Result<()> {
        let next_sequence = writer.sequence.saturating_add(1);
        event.sequence = next_sequence;
        event.session_id.clone_from(&self.session.session_id);
        event.mode = self.session.mode;
        event.emitted_at = Utc::now().to_rfc3339_opts(SecondsFormat::Micros, true);

        let mut encoded = serde_json::to_vec(&event).map_err(io::Error::other)?;
        encoded.push(b'\n');
        writer.file.write_all(&encoded)?;
        writer.file.flush()?;
        writer.sequence = next_sequence;
        Ok(())
    }

    fn log_write_error(&self, error: io::Error) {
        tracing::warn!(%error, path = %self.path.display(), "clean transcript event write failed");
    }
}

/// Path precedence: explicit contract, XDG state, then Codescribe's existing
/// project/data override (`CODESCRIBE_DATA_DIR`) via `Config::config_dir()`.
pub fn transcript_bus_path() -> PathBuf {
    if let Ok(path) = std::env::var(TRANSCRIPT_BUS_PATH_ENV) {
        let path = path.trim();
        if !path.is_empty() {
            return expand_tilde(path);
        }
    }
    if let Ok(root) = std::env::var("XDG_STATE_HOME") {
        let root = root.trim();
        if !root.is_empty() {
            return expand_tilde(root)
                .join("codescribe")
                .join(TRANSCRIPT_BUS_FILENAME);
        }
    }
    codescribe_core::config::Config::config_dir().join(TRANSCRIPT_BUS_FILENAME)
}

fn seconds_to_sample(seconds: f32, sample_rate: u32) -> u64 {
    if !seconds.is_finite() || seconds <= 0.0 {
        return 0;
    }
    (f64::from(seconds) * f64::from(sample_rate)).round() as u64
}

fn expand_tilde(path: &str) -> PathBuf {
    if path == "~" {
        return directories::BaseDirs::new()
            .map(|dirs| dirs.home_dir().to_path_buf())
            .unwrap_or_else(|| PathBuf::from(path));
    }
    if let Some(relative) = path.strip_prefix("~/") {
        return directories::BaseDirs::new()
            .map(|dirs| dirs.home_dir().join(relative))
            .unwrap_or_else(|| PathBuf::from(path));
    }
    PathBuf::from(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bus_flushes_start_commit_and_final_as_private_ndjson() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("events.jsonl");
        let bus = TranscriptBus::open_at(
            TranscriptSession {
                session_id: "session-agent".to_string(),
                mode: TranscriptMode::Agent,
            },
            path.clone(),
            Some(48_000),
        )
        .unwrap();
        bus.publish_utterance(
            "utterance_committed",
            CommittedTranscript {
                utterance_id: 7,
                text: "clean final".to_string(),
                start_seconds: 0.25,
                end_seconds: 1.5,
                segments: Vec::new(),
            },
        );
        bus.publish_final(
            "clean final".to_string(),
            Some("engine-session".to_string()),
        );
        bus.publish_utterance(
            "utterance_revised",
            CommittedTranscript {
                utterance_id: 7,
                text: "must not escape finalization".to_string(),
                start_seconds: 0.25,
                end_seconds: 1.5,
                segments: Vec::new(),
            },
        );
        bus.publish_final("duplicate final".to_string(), None);

        let lines: Vec<CleanTranscriptEvent> = std::fs::read_to_string(&path)
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        assert_eq!(lines.len(), 3);
        assert_eq!(lines[0].status, "session_started");
        assert_eq!(lines[1].status, "utterance_committed");
        assert_eq!(lines[1].sample_start, Some(12_000));
        assert_eq!(lines[1].sample_end, Some(72_000));
        assert_eq!(lines[2].status, "session_finalized");
        assert_eq!(lines[2].text, "clean final");
        assert_eq!(
            lines.iter().map(|event| event.sequence).collect::<Vec<_>>(),
            vec![1, 2, 3]
        );

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(path).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
    }
}
