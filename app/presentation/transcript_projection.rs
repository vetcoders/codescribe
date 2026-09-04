//! Deterministic, metadata-driven projections of the transcript Bus.
//!
//! `rendered_text` is copied into output snapshots but is never inspected to
//! decide identity, ordering, deduplication, replacement, or finality. Those
//! decisions belong exclusively to Bus and reducer metadata.

use std::collections::{HashMap, HashSet};
use std::fmt;
use std::io;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::{Deserialize, Serialize};

use super::transcript_bus::TranscriptProjectionPhase;

pub const LIFECYCLE_SCHEMA: &str = "codescribe.transcript.v1";
pub const EVIDENCE_SCHEMA: &str = "codescribe.transcript-evidence.v1";
pub const PROJECTION_SCHEMA: &str = "codescribe.transcript-projection.v1";

/// The supported Bus row families, decoded without consulting transcript text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TranscriptBusRow {
    Lifecycle(LifecycleRow),
    Evidence(EvidenceRow),
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct LifecycleRow {
    pub schema: String,
    pub sequence: u64,
    pub session_id: String,
    pub status: String,
    #[serde(default)]
    pub phase: Option<TranscriptProjectionPhase>,
    #[serde(default)]
    pub can_paste: bool,
    #[serde(default)]
    pub can_insert: bool,
    #[serde(default)]
    pub can_copy: bool,
    #[serde(default)]
    pub can_retranscribe: bool,
    #[serde(default)]
    pub can_format: bool,
    #[serde(default)]
    pub terminal: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct EvidenceRow {
    pub schema: String,
    pub sequence: u64,
    pub session_id: String,
    pub reducer_revision: u64,
    pub reducer_action: String,
    pub occurrence_session_id: String,
    pub capture_epoch: u64,
    pub sample_start: u64,
    pub sample_end: u64,
    pub document_index: u64,
    pub rendered_text: String,
    #[serde(default)]
    pub phase: TranscriptProjectionPhase,
    #[serde(default)]
    pub can_paste: bool,
    #[serde(default)]
    pub can_insert: bool,
    #[serde(default)]
    pub can_copy: bool,
    #[serde(default)]
    pub can_retranscribe: bool,
    #[serde(default)]
    pub can_format: bool,
    #[serde(default)]
    pub terminal: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TranscriptProjectionKind {
    LiveRevision,
    TerminalSeal,
}

/// One exact reducer snapshot suitable for deterministic JSONL consumers.
///
/// The revision identity deliberately excludes `rendered_text`. A reducer
/// revision produces one projection even though the Bus has one evidence row
/// per document entry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TranscriptProjection {
    pub schema: &'static str,
    pub kind: TranscriptProjectionKind,
    pub session_id: String,
    pub sequence: u64,
    pub reducer_revision: u64,
    pub reducer_action: String,
    pub occurrence_session_id: String,
    pub capture_epoch: u64,
    pub sample_start: u64,
    pub sample_end: u64,
    pub document_index: u64,
    pub rendered_text: String,
    pub phase: TranscriptProjectionPhase,
    pub can_paste: bool,
    pub can_insert: bool,
    pub can_copy: bool,
    pub can_retranscribe: bool,
    pub can_format: bool,
    pub terminal: bool,
}

impl TranscriptProjection {
    pub fn normalized_json(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string(self)
    }
}

#[derive(Debug)]
pub enum ProjectionReadError {
    InvalidUtf8(std::str::Utf8Error),
    InvalidJson(serde_json::Error),
}

impl fmt::Display for ProjectionReadError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidUtf8(error) => write!(formatter, "Bus row is not UTF-8: {error}"),
            Self::InvalidJson(error) => write!(formatter, "Bus row is not valid JSON: {error}"),
        }
    }
}

impl std::error::Error for ProjectionReadError {}

#[derive(Debug, Default)]
struct SessionProjectionState {
    last_sequence: Option<u64>,
    last_reducer_revision: Option<u64>,
    last_evidence: Option<EvidenceRow>,
    terminal_emitted: bool,
}

/// Stateful JSONL decoder and deterministic projection reducer.
#[derive(Debug, Default)]
pub struct TranscriptProjectionReader {
    pending: Vec<u8>,
    current_session: Option<String>,
    retired_sessions: HashSet<String>,
    sessions: HashMap<String, SessionProjectionState>,
}

impl TranscriptProjectionReader {
    pub fn new() -> Self {
        Self::default()
    }

    /// Rotation or truncation starts a new authority domain. No session-local
    /// ordering or deduplication state crosses that boundary.
    pub fn reset_authority(&mut self) {
        self.pending.clear();
        self.current_session = None;
        self.retired_sessions.clear();
        self.sessions.clear();
    }

    /// Feed arbitrary file chunks. Only newline-terminated rows are decoded;
    /// a partial final row remains buffered for the next append.
    pub fn push_bytes(
        &mut self,
        bytes: &[u8],
    ) -> Vec<Result<TranscriptProjection, ProjectionReadError>> {
        self.pending.extend_from_slice(bytes);
        let mut output = Vec::new();
        while let Some(newline) = self.pending.iter().position(|byte| *byte == b'\n') {
            let line: Vec<u8> = self.pending.drain(..=newline).collect();
            let line = &line[..line.len().saturating_sub(1)];
            if line.is_empty() {
                continue;
            }
            match std::str::from_utf8(line) {
                Ok(line) => match self.push_line(line) {
                    Ok(Some(projection)) => output.push(Ok(projection)),
                    Ok(None) => {}
                    Err(error) => output.push(Err(error)),
                },
                Err(error) => output.push(Err(ProjectionReadError::InvalidUtf8(error))),
            }
        }
        output
    }

    pub fn push_line(
        &mut self,
        line: &str,
    ) -> Result<Option<TranscriptProjection>, ProjectionReadError> {
        let value: serde_json::Value =
            serde_json::from_str(line).map_err(ProjectionReadError::InvalidJson)?;
        let schema = value
            .get("schema")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default();
        let row = match schema {
            LIFECYCLE_SCHEMA => TranscriptBusRow::Lifecycle(
                serde_json::from_value(value).map_err(ProjectionReadError::InvalidJson)?,
            ),
            EVIDENCE_SCHEMA => TranscriptBusRow::Evidence(
                serde_json::from_value(value).map_err(ProjectionReadError::InvalidJson)?,
            ),
            _ => return Ok(None),
        };
        Ok(self.project(row))
    }

    pub fn project(&mut self, row: TranscriptBusRow) -> Option<TranscriptProjection> {
        match row {
            TranscriptBusRow::Lifecycle(row) => self.observe_lifecycle(row),
            TranscriptBusRow::Evidence(row) => self.project_evidence(row),
        }
    }

    fn observe_lifecycle(&mut self, row: LifecycleRow) -> Option<TranscriptProjection> {
        if row.status == "session_started" {
            if !self.select_session(&row.session_id) {
                return None;
            }
        } else if self.current_session.as_deref() != Some(row.session_id.as_str()) {
            return None;
        }
        if !self.accept_sequence(&row.session_id, row.sequence) || row.status != "session_ended" {
            return None;
        }

        let state = self.sessions.entry(row.session_id.clone()).or_default();
        if std::mem::replace(&mut state.terminal_emitted, true) {
            return None;
        }
        let last = state.last_evidence.clone();
        self.current_session = None;
        self.retired_sessions.insert(row.session_id.clone());

        let has_text = last
            .as_ref()
            .is_some_and(|evidence| !evidence.rendered_text.trim().is_empty());
        let legacy_lifecycle = row.phase.is_none();
        let phase = row.phase.unwrap_or(if has_text {
            TranscriptProjectionPhase::Formatted
        } else {
            TranscriptProjectionPhase::NoSpeech
        });
        let can_copy = if legacy_lifecycle {
            has_text
        } else {
            row.can_copy
        };
        let can_format = if legacy_lifecycle {
            has_text
        } else {
            row.can_format
        };

        Some(match last {
            Some(last) => TranscriptProjection {
                schema: PROJECTION_SCHEMA,
                kind: TranscriptProjectionKind::TerminalSeal,
                session_id: row.session_id,
                sequence: row.sequence,
                reducer_revision: last.reducer_revision,
                reducer_action: "session_ended".to_string(),
                occurrence_session_id: last.occurrence_session_id,
                capture_epoch: last.capture_epoch,
                sample_start: last.sample_start,
                sample_end: last.sample_end,
                document_index: last.document_index,
                rendered_text: last.rendered_text,
                phase,
                can_paste: row.can_paste,
                can_insert: row.can_insert,
                can_copy,
                can_retranscribe: row.can_retranscribe,
                can_format,
                terminal: true,
            },
            None => TranscriptProjection {
                schema: PROJECTION_SCHEMA,
                kind: TranscriptProjectionKind::TerminalSeal,
                session_id: row.session_id,
                sequence: row.sequence,
                reducer_revision: 0,
                reducer_action: "session_ended".to_string(),
                occurrence_session_id: String::new(),
                capture_epoch: 0,
                sample_start: 0,
                sample_end: 0,
                document_index: 0,
                rendered_text: String::new(),
                phase,
                can_paste: row.can_paste,
                can_insert: row.can_insert,
                can_copy,
                can_retranscribe: row.can_retranscribe,
                can_format,
                terminal: true,
            },
        })
    }

    fn project_evidence(&mut self, row: EvidenceRow) -> Option<TranscriptProjection> {
        if !self.select_session(&row.session_id)
            || !self.accept_sequence(&row.session_id, row.sequence)
        {
            return None;
        }

        let state = self.sessions.entry(row.session_id.clone()).or_default();
        if state
            .last_reducer_revision
            .is_some_and(|revision| row.reducer_revision < revision)
        {
            return None;
        }
        state.last_reducer_revision = Some(
            state
                .last_reducer_revision
                .map_or(row.reducer_revision, |revision| {
                    revision.max(row.reducer_revision)
                }),
        );
        state.last_evidence = Some(row.clone());
        Some(TranscriptProjection {
            schema: PROJECTION_SCHEMA,
            kind: TranscriptProjectionKind::LiveRevision,
            session_id: row.session_id,
            sequence: row.sequence,
            reducer_revision: row.reducer_revision,
            reducer_action: row.reducer_action,
            occurrence_session_id: row.occurrence_session_id,
            capture_epoch: row.capture_epoch,
            sample_start: row.sample_start,
            sample_end: row.sample_end,
            document_index: row.document_index,
            rendered_text: row.rendered_text,
            phase: row.phase,
            can_paste: row.can_paste,
            can_insert: row.can_insert,
            can_copy: row.can_copy,
            can_retranscribe: row.can_retranscribe,
            can_format: row.can_format,
            terminal: row.terminal,
        })
    }

    fn select_session(&mut self, session_id: &str) -> bool {
        if self.current_session.as_deref() == Some(session_id) {
            return true;
        }
        if self.retired_sessions.contains(session_id) {
            return false;
        }
        if let Some(previous) = self.current_session.replace(session_id.to_string()) {
            self.retired_sessions.insert(previous);
        }
        true
    }

    fn accept_sequence(&mut self, session_id: &str, sequence: u64) -> bool {
        let state = self.sessions.entry(session_id.to_string()).or_default();
        if state
            .last_sequence
            .is_some_and(|last_sequence| sequence <= last_sequence)
        {
            return false;
        }
        state.last_sequence = Some(sequence);
        true
    }
}

/// Why a file wait returned. Timeout is only a bounded recovery path for a
/// missed/replaced watch, never the primary macOS trigger.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileWakeReason {
    FileEvent,
    RecoveryTimeout,
}

/// Platform file-event wake for live Bus consumers.
pub struct TranscriptBusFileWake {
    inner: file_wake::PlatformFileWake,
}

impl TranscriptBusFileWake {
    pub fn new(path: impl AsRef<Path>) -> io::Result<Self> {
        Ok(Self {
            inner: file_wake::PlatformFileWake::new(path.as_ref().to_path_buf())?,
        })
    }

    pub fn wait(&mut self, recovery_timeout: Duration) -> io::Result<FileWakeReason> {
        self.inner.wait(recovery_timeout)
    }
}

#[cfg(target_os = "macos")]
mod file_wake {
    use super::{Duration, FileWakeReason, PathBuf, io};
    use std::fs::File;
    use std::mem::MaybeUninit;
    use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
    use std::os::unix::fs::MetadataExt;
    use std::ptr;

    pub struct PlatformFileWake {
        path: PathBuf,
        queue: OwnedFd,
        target: Option<File>,
        target_identity: Option<(u64, u64)>,
    }

    impl PlatformFileWake {
        pub fn new(path: PathBuf) -> io::Result<Self> {
            let queue_fd = unsafe { libc::kqueue() };
            if queue_fd < 0 {
                return Err(io::Error::last_os_error());
            }
            let queue = unsafe { OwnedFd::from_raw_fd(queue_fd) };
            let mut wake = Self {
                path,
                queue,
                target: None,
                target_identity: None,
            };
            wake.refresh_target()?;
            Ok(wake)
        }

        pub fn wait(&mut self, recovery_timeout: Duration) -> io::Result<FileWakeReason> {
            self.refresh_target()?;
            let timeout = libc::timespec {
                tv_sec: recovery_timeout
                    .as_secs()
                    .try_into()
                    .unwrap_or(libc::time_t::MAX),
                tv_nsec: recovery_timeout.subsec_nanos().into(),
            };
            let mut events: [libc::kevent; 4] = unsafe { MaybeUninit::zeroed().assume_init() };
            let count = unsafe {
                libc::kevent(
                    self.queue.as_raw_fd(),
                    ptr::null(),
                    0,
                    events.as_mut_ptr(),
                    events.len() as i32,
                    &timeout,
                )
            };
            if count < 0 {
                return Err(io::Error::last_os_error());
            }
            if count == 0 {
                Ok(FileWakeReason::RecoveryTimeout)
            } else {
                Ok(FileWakeReason::FileEvent)
            }
        }

        fn refresh_target(&mut self) -> io::Result<()> {
            let file = match File::open(&self.path) {
                Ok(file) => file,
                Err(error) if error.kind() == io::ErrorKind::NotFound => {
                    self.target = None;
                    self.target_identity = None;
                    return Ok(());
                }
                Err(error) => return Err(error),
            };
            let metadata = file.metadata()?;
            let identity = (metadata.dev(), metadata.ino());
            if self.target_identity == Some(identity) {
                return Ok(());
            }
            self.register(file.as_raw_fd())?;
            self.target = Some(file);
            self.target_identity = Some(identity);
            Ok(())
        }

        fn register(&self, descriptor: i32) -> io::Result<()> {
            let change = libc::kevent {
                ident: descriptor as libc::uintptr_t,
                filter: libc::EVFILT_VNODE,
                flags: libc::EV_ADD | libc::EV_CLEAR,
                fflags: libc::NOTE_WRITE
                    | libc::NOTE_EXTEND
                    | libc::NOTE_ATTRIB
                    | libc::NOTE_DELETE
                    | libc::NOTE_RENAME,
                data: 0,
                udata: ptr::null_mut(),
            };
            let result = unsafe {
                libc::kevent(
                    self.queue.as_raw_fd(),
                    &change,
                    1,
                    ptr::null_mut(),
                    0,
                    ptr::null(),
                )
            };
            if result < 0 {
                Err(io::Error::last_os_error())
            } else {
                Ok(())
            }
        }
    }
}

#[cfg(not(target_os = "macos"))]
mod file_wake {
    use super::{Duration, FileWakeReason, PathBuf, io};

    pub struct PlatformFileWake {
        _path: PathBuf,
    }

    impl PlatformFileWake {
        pub fn new(path: PathBuf) -> io::Result<Self> {
            Ok(Self { _path: path })
        }

        pub fn wait(&mut self, recovery_timeout: Duration) -> io::Result<FileWakeReason> {
            std::thread::sleep(recovery_timeout);
            Ok(FileWakeReason::RecoveryTimeout)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lifecycle(session: &str, sequence: u64, status: &str) -> String {
        serde_json::json!({
            "schema": LIFECYCLE_SCHEMA,
            "sequence": sequence,
            "session_id": session,
            "status": status,
            "text": "lifecycle text is never projection authority"
        })
        .to_string()
    }

    fn evidence(
        session: &str,
        sequence: u64,
        revision: u64,
        action: &str,
        rendered_text: &str,
    ) -> String {
        serde_json::json!({
            "schema": EVIDENCE_SCHEMA,
            "sequence": sequence,
            "session_id": session,
            "reducer_revision": revision,
            "reducer_action": action,
            "occurrence_session_id": session,
            "capture_epoch": 1,
            "sample_start": sequence * 100,
            "sample_end": sequence * 100 + 99,
            "document_index": sequence - 1,
            "rendered_text": rendered_text
        })
        .to_string()
    }

    fn replay(input: &str) -> Vec<String> {
        let mut reader = TranscriptProjectionReader::new();
        reader
            .push_bytes(input.as_bytes())
            .into_iter()
            .map(|projection| {
                projection
                    .expect("valid row")
                    .normalized_json()
                    .expect("serializable projection")
            })
            .collect()
    }

    #[test]
    fn identical_jsonl_replay_has_byte_identical_normalized_output() {
        let input = [
            lifecycle("s1", 1, "session_started"),
            evidence("s1", 2, 1, "apply_ledger_decision", "alfa"),
            evidence("s1", 3, 2, "record_ledger_terminal_seal", "alfa beta"),
            lifecycle("s1", 4, "session_ended"),
        ]
        .join("\n")
            + "\n";
        assert_eq!(replay(&input), replay(&input));
    }

    #[test]
    fn unrelated_newer_text_replaces_by_revision_without_prefix_inference() {
        let input = [
            evidence("s1", 1, 7, "apply_ledger_decision", "pierwszy dokument"),
            evidence(
                "s1",
                2,
                8,
                "apply_ledger_decision",
                "Żółw przepisuje całość",
            ),
        ]
        .join("\n")
            + "\n";
        let output = replay(&input);
        assert_eq!(output.len(), 2);
        let second: serde_json::Value =
            serde_json::from_str(&output[1]).expect("normalized projection is JSON");
        assert_eq!(second["rendered_text"], "Żółw przepisuje całość");
        assert_eq!(second["reducer_revision"], 8);
    }

    #[test]
    fn equal_text_on_distinct_occurrences_survives_but_terminal_phase_emits_once() {
        let input = [
            evidence("s1", 1, 1, "apply_ledger_decision", "Iwo Iwo"),
            evidence("s1", 2, 2, "apply_ledger_decision", "Iwo Iwo"),
            evidence("s1", 3, 2, "apply_ledger_decision", "Iwo Iwo"),
            evidence("s1", 3, 2, "apply_ledger_decision", "Iwo Iwo"),
            evidence("s1", 4, 3, "record_ledger_terminal_seal", "Iwo Iwo"),
            evidence("s1", 5, 3, "record_ledger_terminal_seal", "Iwo Iwo"),
            lifecycle("s1", 6, "session_ended"),
        ]
        .join("\n")
            + "\n";
        let output = replay(&input);
        assert_eq!(output.len(), 6);
        assert!(output[0].contains("\"reducer_revision\":1"));
        assert!(output[1].contains("\"reducer_revision\":2"));
        assert!(output[2].contains("\"reducer_revision\":2"));
        assert!(output[4].contains("\"kind\":\"live_revision\""));
        assert!(output[5].contains("\"kind\":\"terminal_seal\""));
        assert!(output[5].contains("\"reducer_action\":\"session_ended\""));
    }

    #[test]
    fn lifecycle_and_late_prior_session_rows_cannot_displace_newer_truth() {
        let input = [
            lifecycle("old", 1, "session_started"),
            evidence("old", 2, 1, "record_ledger_terminal_seal", "stare"),
            lifecycle("old", 3, "session_ended"),
            lifecycle("new", 1, "session_started"),
            evidence("new", 2, 1, "apply_ledger_decision", "nowe"),
            lifecycle("new", 3, "session_ended"),
            evidence(
                "old",
                4,
                2,
                "record_ledger_terminal_seal",
                "spóźnione stare",
            ),
        ]
        .join("\n")
            + "\n";
        let output = replay(&input);
        assert_eq!(output.len(), 4);
        assert!(output.last().unwrap().contains("\"session_id\":\"new\""));
        assert!(
            output
                .last()
                .unwrap()
                .contains("\"rendered_text\":\"nowe\"")
        );
        assert!(
            output
                .last()
                .unwrap()
                .contains("\"kind\":\"terminal_seal\"")
        );
    }

    #[test]
    fn truncation_reset_starts_a_fresh_authority_domain() {
        let row = evidence("s1", 1, 1, "apply_ledger_decision", "alfa") + "\n";
        let mut reader = TranscriptProjectionReader::new();
        assert_eq!(reader.push_bytes(row.as_bytes()).len(), 1);
        assert!(reader.push_bytes(row.as_bytes()).is_empty());
        reader.reset_authority();
        assert_eq!(reader.push_bytes(row.as_bytes()).len(), 1);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn macos_file_event_wake_observes_a_real_append_within_the_bound() {
        use std::io::Write as _;

        let directory = tempfile::tempdir().expect("temporary directory");
        let path = directory.path().join("bus.jsonl");
        std::fs::File::create(&path).expect("create Bus witness file");
        let mut wake = TranscriptBusFileWake::new(&path).expect("arm file wake");
        let append_path = path.clone();
        let writer = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(50));
            let mut file = std::fs::OpenOptions::new()
                .append(true)
                .open(append_path)
                .expect("open Bus witness file");
            writeln!(file, "witness").expect("append witness");
            file.flush().expect("flush witness");
        });
        let started = std::time::Instant::now();
        let reason = wake
            .wait(Duration::from_secs(2))
            .expect("wait for file event");
        writer.join().expect("append thread");
        assert_eq!(reason, FileWakeReason::FileEvent);
        assert!(started.elapsed() < Duration::from_secs(2));
    }
}
