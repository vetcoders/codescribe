//! Durable clean transcript events for operator and control-plane consumers.
//!
//! The bus observes the mutable [`PresentationEmitter`] draft and the one
//! authoritative product seal chosen by [`crate::controller::RecordingController`].
//! It never opens audio, re-transcribes a file, or reconstructs text from UI
//! deltas. One append-only JSON object is flushed per state transition.

use std::collections::BTreeMap;
use std::fs::{File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use chrono::{SecondsFormat, Utc};
use codescribe_core::audio::capture_receipt::session_energy_db;
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

/// One mutable utterance slot in the live transcript draft.
#[derive(Debug, Clone, PartialEq)]
pub struct TranscriptDraft {
    pub utterance_id: u64,
    pub text: String,
    pub start_seconds: f32,
    pub end_seconds: f32,
    pub segments: Vec<TranscriptSegment>,
}

/// Typed draft transition. Product truth is never represented by this enum;
/// only [`TranscriptBus::publish_sealed`] can cross the immutable boundary.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TranscriptDraftStatus {
    Created,
    Revised,
}

impl TranscriptDraftStatus {
    fn as_str(self) -> &'static str {
        match self {
            Self::Created => "utterance_draft",
            Self::Revised => "utterance_revised",
        }
    }
}

/// Grain of one published span. Word pins are engine evidence; utterance
/// grain is the honest fallback when Apple committed a window, not words.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TranscriptWordGrain {
    #[default]
    Word,
    Utterance,
}

fn is_word_grain(grain: &TranscriptWordGrain) -> bool {
    matches!(grain, TranscriptWordGrain::Word)
}

/// One span on the capture PCM clock: text + samples + intensity.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TranscriptWordSpan {
    pub text: String,
    pub sample_start: u64,
    pub sample_end: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub energy_db: Option<f32>,
    #[serde(default, skip_serializing_if = "is_word_grain")]
    pub grain: TranscriptWordGrain,
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
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub words: Vec<TranscriptWordSpan>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pipeline_session_id: Option<String>,
}

/// Synchronous low-frequency writer. Draft boundaries occur on the STT worker,
/// not the CoreAudio callback, and each line is flushed so a live tailer sees
/// them before the next utterance or process exit.
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
    sealed: bool,
    drafts: BTreeMap<u64, TranscriptDraft>,
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
                sealed: false,
                drafts: BTreeMap::new(),
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

    /// Publish a new mutable utterance slot or a bounded revision of that slot.
    pub fn publish_draft(&self, status: TranscriptDraftStatus, utterance: TranscriptDraft) {
        let status = status.as_str();
        let sample_rate = self.sample_rate();
        let words = word_spans_from_draft(&utterance, sample_rate);
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
            text: utterance.text.clone(),
            segments: utterance.segments.clone(),
            words,
            pipeline_session_id: None,
        };

        let mut writer = self
            .writer
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if writer.sealed {
            tracing::warn!(session_id = %self.session.session_id, %status, "transcript draft ignored after product seal");
            return;
        }
        writer.drafts.insert(utterance.utterance_id, utterance);
        if let Err(error) = self
            .ensure_started_locked(&mut writer)
            .and_then(|_| self.write_event_locked(&mut writer, event))
        {
            self.log_write_error(error);
        }
    }

    /// Publish the one immutable product truth after every configured automatic
    /// stage (engine layers, final pass, adjudication, postprocess, formatting)
    /// has completed. The first call wins byte-for-byte; later calls are ignored.
    pub fn publish_sealed(&self, text: String, pipeline_session_id: Option<String>) {
        let mut writer = self
            .writer
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if writer.sealed {
            return;
        }
        let sample_rate = self.sample_rate();
        let clock = aggregate_seal_clock(&writer.drafts, sample_rate);
        let event = CleanTranscriptEvent {
            schema: "codescribe.transcript.v1".to_string(),
            sequence: 0,
            session_id: String::new(),
            mode: self.session.mode,
            utterance_id: None,
            emitted_at: String::new(),
            status: "transcript_sealed".to_string(),
            sample_rate_hz: sample_rate,
            sample_start: clock.sample_start,
            sample_end: clock.sample_end,
            audio_start_seconds: clock.audio_start_seconds,
            audio_end_seconds: clock.audio_end_seconds,
            text,
            segments: clock.segments,
            words: clock.words,
            pipeline_session_id,
        };
        match self
            .ensure_started_locked(&mut writer)
            .and_then(|_| self.write_event_locked(&mut writer, event))
        {
            Ok(()) => writer.sealed = true,
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
                words: Vec::new(),
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

fn finite_audio_window(start: f32, end: f32) -> Option<(f32, f32)> {
    if start.is_finite() && end.is_finite() && end > start {
        Some((start, end))
    } else {
        None
    }
}

fn word_span_from_seconds(
    text: String,
    start: f32,
    end: f32,
    sample_rate: Option<u32>,
    grain: TranscriptWordGrain,
) -> Option<TranscriptWordSpan> {
    let (start, end) = finite_audio_window(start, end)?;
    if text.trim().is_empty() {
        return None;
    }
    let rate = sample_rate.filter(|rate| *rate > 0)?;
    let sample_start = seconds_to_sample(start, rate);
    let sample_end = seconds_to_sample(end, rate).max(sample_start.saturating_add(1));
    Some(TranscriptWordSpan {
        text,
        sample_start,
        sample_end,
        energy_db: session_energy_db(sample_start, sample_end),
        grain,
    })
}

fn word_spans_from_draft(
    utterance: &TranscriptDraft,
    sample_rate: Option<u32>,
) -> Vec<TranscriptWordSpan> {
    if !utterance.segments.is_empty() {
        return utterance
            .segments
            .iter()
            .filter_map(|segment| {
                word_span_from_seconds(
                    segment.text.clone(),
                    segment.start_ts,
                    segment.end_ts,
                    sample_rate,
                    TranscriptWordGrain::Word,
                )
            })
            .collect();
    }
    word_span_from_seconds(
        utterance.text.clone(),
        utterance.start_seconds,
        utterance.end_seconds,
        sample_rate,
        TranscriptWordGrain::Utterance,
    )
    .into_iter()
    .collect()
}

struct SealClock {
    sample_start: Option<u64>,
    sample_end: Option<u64>,
    audio_start_seconds: Option<f32>,
    audio_end_seconds: Option<f32>,
    segments: Vec<TranscriptSegment>,
    words: Vec<TranscriptWordSpan>,
}

fn aggregate_seal_clock(
    drafts: &BTreeMap<u64, TranscriptDraft>,
    sample_rate: Option<u32>,
) -> SealClock {
    let mut audio_start = None;
    let mut audio_end = None;
    let mut segments = Vec::new();
    let mut words = Vec::new();
    for draft in drafts.values() {
        if let Some((start, end)) = finite_audio_window(draft.start_seconds, draft.end_seconds) {
            audio_start = Some(audio_start.map_or(start, |seen: f32| seen.min(start)));
            audio_end = Some(audio_end.map_or(end, |seen: f32| seen.max(end)));
        }
        segments.extend(draft.segments.iter().cloned());
        words.extend(word_spans_from_draft(draft, sample_rate));
    }
    SealClock {
        sample_start: match (sample_rate, audio_start) {
            (Some(rate), Some(start)) => Some(seconds_to_sample(start, rate)),
            _ => None,
        },
        sample_end: match (sample_rate, audio_end) {
            (Some(rate), Some(end)) => Some(seconds_to_sample(end, rate)),
            _ => None,
        },
        audio_start_seconds: audio_start,
        audio_end_seconds: audio_end,
        segments,
        words,
    }
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
    fn bus_flushes_start_draft_and_seal_as_private_ndjson() {
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
        bus.publish_draft(
            TranscriptDraftStatus::Created,
            TranscriptDraft {
                utterance_id: 7,
                text: "clean final".to_string(),
                start_seconds: 0.25,
                end_seconds: 1.5,
                segments: Vec::new(),
            },
        );
        bus.publish_sealed(
            "clean final".to_string(),
            Some("engine-session".to_string()),
        );
        bus.publish_draft(
            TranscriptDraftStatus::Revised,
            TranscriptDraft {
                utterance_id: 7,
                text: "must not escape finalization".to_string(),
                start_seconds: 0.25,
                end_seconds: 1.5,
                segments: Vec::new(),
            },
        );
        bus.publish_sealed("duplicate final".to_string(), None);

        let lines: Vec<CleanTranscriptEvent> = std::fs::read_to_string(&path)
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        assert_eq!(lines.len(), 3);
        assert_eq!(lines[0].status, "session_started");
        assert_eq!(lines[1].status, "utterance_draft");
        assert_eq!(lines[1].sample_start, Some(12_000));
        assert_eq!(lines[1].sample_end, Some(72_000));
        assert_eq!(lines[2].status, "transcript_sealed");
        assert_eq!(lines[2].text, "clean final");
        assert_eq!(lines[2].sample_start, Some(12_000));
        assert_eq!(lines[2].sample_end, Some(72_000));
        assert_eq!(lines[2].audio_start_seconds, Some(0.25));
        assert_eq!(lines[2].audio_end_seconds, Some(1.5));
        assert_eq!(lines[2].words.len(), 1);
        assert_eq!(lines[2].words[0].sample_start, 12_000);
        assert_eq!(lines[2].words[0].sample_end, 72_000);
        assert_eq!(lines[2].words[0].grain, TranscriptWordGrain::Utterance);
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

    #[test]
    fn seal_publishes_word_spans_on_the_pcm_clock() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("words.jsonl");
        let bus = TranscriptBus::open_at(
            TranscriptSession {
                session_id: "session-words".to_string(),
                mode: TranscriptMode::Dictation,
            },
            path.clone(),
            Some(16_000),
        )
        .unwrap();
        bus.publish_draft(
            TranscriptDraftStatus::Created,
            TranscriptDraft {
                utterance_id: 3,
                text: "dwa slowa".to_string(),
                start_seconds: 1.0,
                end_seconds: 2.0,
                segments: vec![
                    TranscriptSegment {
                        text: "dwa".to_string(),
                        start_ts: 1.0,
                        end_ts: 1.4,
                    },
                    TranscriptSegment {
                        text: "slowa".to_string(),
                        start_ts: 1.4,
                        end_ts: 2.0,
                    },
                ],
            },
        );
        bus.publish_sealed("dwa slowa".to_string(), None);

        let lines: Vec<CleanTranscriptEvent> = std::fs::read_to_string(&path)
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        let seal = lines
            .iter()
            .find(|event| event.status == "transcript_sealed")
            .expect("seal");
        assert_eq!(seal.sample_start, Some(16_000));
        assert_eq!(seal.sample_end, Some(32_000));
        assert_eq!(seal.segments.len(), 2);
        assert_eq!(seal.words.len(), 2);
        assert_eq!(seal.words[0].text, "dwa");
        assert_eq!(seal.words[0].sample_start, 16_000);
        assert_eq!(seal.words[0].sample_end, 22_400);
        assert_eq!(seal.words[0].grain, TranscriptWordGrain::Word);
        assert_eq!(seal.words[1].text, "slowa");
        assert_eq!(seal.words[1].sample_start, 22_400);
        assert_eq!(seal.words[1].sample_end, 32_000);
    }
}
