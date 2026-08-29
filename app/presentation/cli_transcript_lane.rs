//! CLI file-transcription lane on the clean transcript bus.
//!
//! WHY THIS IS NOT A METHOD ON [`TranscriptBus`]. That type is the throne's
//! observer: it copies occurrence-authenticated ledger revisions and its module
//! doc promises it never "accepts arbitrary text" or "re-transcribes a file".
//! A CLI file pass is exactly the thing that promise excludes — it has no
//! ledger, no acoustic receipts, and no occurrence identity. Rather than
//! loosening that promise (which the whole one-throne plan exists to protect),
//! the CLI gets its own writer on the same NDJSON shape, and every event it
//! writes names itself.
//!
//! Wire compatibility is deliberate: these are `codescribe.transcript.v1`
//! events with the statuses `scripts/bus-demux.py` and
//! `codescribe transcribe live` already read (`utterance_draft`,
//! `transcript_sealed`). What separates them from app events is the additive
//! `source` field — absent on everything the app writes, `"cli_file_verdict"`
//! here. A consumer that wants ledger-backed truth filters on its absence; a
//! consumer that just wants the operator's words reads both.

use std::fs::OpenOptions;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use chrono::{SecondsFormat, Utc};
use codescribe_core::pipeline::contracts::TranscriptSegment;

use super::transcript_bus::{
    CleanTranscriptEvent, TranscriptMode, TranscriptSessionEndReason, transcript_bus_path,
};

/// Value of [`CleanTranscriptEvent::source`] on every event this lane writes.
/// Its absence means "written by the app"; nothing else may claim this string.
pub const CLI_FILE_VERDICT_SOURCE: &str = "cli_file_verdict";

/// Append-only publisher for one `codescribe transcribe <file>` run.
///
/// One instance owns one session. It holds no lock beyond the file's O_APPEND
/// semantics: the app may be writing the same bus concurrently, and NDJSON
/// lines under the append flag do not interleave at these sizes.
pub struct CliTranscriptLane {
    session_id: String,
    mode: TranscriptMode,
    path: PathBuf,
    sequence: u64,
    /// Utterance numbering is its own axis, exactly as in the app: drafts are
    /// 1, 2, 3… while `sequence` counts every line including lifecycle ones.
    /// Deriving one from the other would make `utterance_id` skip.
    utterance_counter: u64,
    started: bool,
    ended: bool,
}

impl CliTranscriptLane {
    /// Open the configured bus. `None` when the bus is unavailable — a CLI
    /// transcription must still print its result when the bus cannot be
    /// written, so callers treat this as optional, never fatal.
    pub fn open(session_id: String, mode: TranscriptMode) -> Option<Self> {
        Self::open_at(session_id, mode, transcript_bus_path()).ok()
    }

    pub fn open_at(session_id: String, mode: TranscriptMode, path: PathBuf) -> io::Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        Ok(Self {
            session_id,
            mode,
            path,
            sequence: 0,
            utterance_counter: 0,
            started: false,
            ended: false,
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn session_id(&self) -> &str {
        &self.session_id
    }

    /// Write the session-start line exactly once, text-free, mirroring the
    /// app's own lifecycle so a tailer can bracket this run.
    pub fn publish_started(&mut self) -> io::Result<()> {
        if self.started {
            return Ok(());
        }
        self.write(self.lifecycle("session_started", None))?;
        self.started = true;
        Ok(())
    }

    /// Publish ONE decoded segment as its own utterance draft.
    ///
    /// The grain matters and it is not a style choice. Measured against a real
    /// app session's drafts, `utterance_draft` carries a single self-contained
    /// utterance with its own `utterance_id` — a short phrase, one per decoded
    /// unit — never the growing document. Publishing the accumulated text
    /// makes every append-only tailer (`codescribe transcribe live`,
    /// `scripts/bus-demux.py`) reprint the whole transcript once per segment,
    /// which is quadratic noise in a terminal and a false claim that the
    /// utterance got longer.
    pub fn publish_draft(&mut self, text: &str, segment: &TranscriptSegment) -> io::Result<()> {
        self.publish_started()?;
        self.utterance_counter = self.utterance_counter.saturating_add(1);
        let mut event = self.lifecycle("utterance_draft", Some(self.utterance_counter));
        event.text = text.to_string();
        event.audio_start_seconds = Some(segment.start_ts);
        event.audio_end_seconds = Some(segment.end_ts);
        self.write(event)
    }

    /// The one definition of "a printable utterance" in a decoded file: the
    /// segment's own trimmed text, empties dropped. Both the bus lane and the
    /// `--stream` stdout read from here so they cannot drift apart.
    pub fn segment_texts(segments: &[TranscriptSegment]) -> Vec<String> {
        segments
            .iter()
            .filter_map(|segment| {
                let text = segment.text.trim();
                (!text.is_empty()).then(|| text.to_string())
            })
            .collect()
    }

    /// Publish every decoded segment as its own draft, in order, and return the
    /// texts the caller should print — one line per utterance.
    ///
    /// This loop lives here, and not at the call site, on purpose. When the
    /// caller owned it, it handed `publish_draft` the running document instead
    /// of the segment, and no test driving the lane could see that: the lane
    /// faithfully wrote whatever it was given, under the right status and with
    /// a plausible `utterance_id`. Owning the iteration makes the accumulation
    /// unreachable from any call site, so a test of THIS method witnesses the
    /// effect a bus tailer actually gets.
    pub fn publish_segments(&mut self, segments: &[TranscriptSegment]) -> io::Result<Vec<String>> {
        let spoken = Self::segment_texts(segments);
        for (text, segment) in spoken.iter().zip(
            segments
                .iter()
                .filter(|segment| !segment.text.trim().is_empty()),
        ) {
            self.publish_draft(text, segment)?;
        }
        Ok(spoken)
    }

    /// Publish the whole file verdict once. `transcript_sealed` here means what
    /// it means everywhere on this lane — "this document is final, not a draft"
    /// — and `source` says who finalised it. It is not, and must not be read
    /// as, a ledger seal.
    pub fn publish_sealed(&mut self, text: &str, segments: &[TranscriptSegment]) -> io::Result<()> {
        self.publish_started()?;
        let mut event = self.lifecycle("transcript_sealed", None);
        event.text = text.to_string();
        event.segments = segments.to_vec();
        event.audio_start_seconds = segments.first().map(|segment| segment.start_ts);
        event.audio_end_seconds = segments.last().map(|segment| segment.end_ts);
        self.write(event)
    }

    /// Copy the file this lane decoded to `sessions/<session_id>.wav`.
    ///
    /// Demux identity is that path. `last_session.wav` is a latest-app-take
    /// alias and is never written here — a CLI re-decode of an old file must
    /// not pretend to be the last live take.
    pub fn retain_source_wav(&self, source: &Path) -> io::Result<PathBuf> {
        self.retain_source_wav_at(
            source,
            &codescribe_core::config::Config::config_dir().join("sessions"),
        )
    }

    pub fn retain_source_wav_at(&self, source: &Path, sessions_dir: &Path) -> io::Result<PathBuf> {
        let id = self.session_id.as_str();
        let safe = (8..=80).contains(&id.len())
            && id
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
        if !safe {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "session id is not a safe wav filename",
            ));
        }
        let meta = std::fs::metadata(source)?;
        if !meta.is_file() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "CLI source wav must be a regular file",
            ));
        }
        std::fs::create_dir_all(sessions_dir)?;
        let dest = sessions_dir.join(format!("{id}.wav"));
        if dest.file_name().and_then(|name| name.to_str()) == Some("last_session.wav") {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "CLI file verdict must not retain last_session.wav as identity",
            ));
        }
        if source == dest {
            return Ok(dest);
        }
        // Destination name is only the validated session alphabet; source is
        // the same regular file this CLI run already decoded.
        // nosemgrep: rust.actix.path-traversal.tainted-path.tainted-path -- dest is sessions_dir + safe session id; source metadata-checked as a regular file.
        std::fs::copy(source, &dest)?;
        Ok(dest)
    }

    /// Close the session. Like the app's bus, this never writes a terminal line
    /// for a session that never started, so `session_started` without
    /// `session_ended` still means "this run did not finish".
    pub fn publish_ended(&mut self, reason: TranscriptSessionEndReason) -> io::Result<()> {
        if !self.started || self.ended {
            return Ok(());
        }
        let mut event = self.lifecycle("session_ended", None);
        event.end_reason = Some(reason);
        self.write(event)?;
        self.ended = true;
        Ok(())
    }

    fn lifecycle(&self, status: &str, utterance_id: Option<u64>) -> CleanTranscriptEvent {
        CleanTranscriptEvent {
            schema: "codescribe.transcript.v1".to_string(),
            sequence: 0,
            session_id: self.session_id.clone(),
            mode: self.mode,
            utterance_id,
            emitted_at: String::new(),
            status: status.to_string(),
            sample_rate_hz: None,
            capture_epoch: None,
            // A file pass owns seconds, not the capture PCM axis. Claiming
            // sample offsets here would fabricate an occurrence identity.
            sample_start: None,
            sample_end: None,
            audio_start_seconds: None,
            audio_end_seconds: None,
            text: String::new(),
            segments: Vec::new(),
            words: Vec::new(),
            coverage: None,
            pipeline_session_id: None,
            end_reason: None,
            source: Some(CLI_FILE_VERDICT_SOURCE.to_string()),
        }
    }

    fn write(&mut self, mut event: CleanTranscriptEvent) -> io::Result<()> {
        let next = self.sequence.saturating_add(1);
        event.sequence = next;
        event.emitted_at = Utc::now().to_rfc3339_opts(SecondsFormat::Micros, true);

        let mut options = OpenOptions::new();
        options.create(true).append(true).write(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(&self.path)?;

        let mut encoded = serde_json::to_vec(&event).map_err(io::Error::other)?;
        encoded.push(b'\n');
        file.write_all(&encoded)?;
        file.flush()?;
        self.sequence = next;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn segment(text: &str, start: f32, end: f32) -> TranscriptSegment {
        TranscriptSegment {
            text: text.to_string(),
            start_ts: start,
            end_ts: end,
        }
    }

    fn read(path: &Path) -> Vec<CleanTranscriptEvent> {
        std::fs::read_to_string(path)
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect()
    }

    #[test]
    fn every_cli_event_names_itself_so_ledger_truth_stays_separable() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("events.jsonl");
        let mut lane =
            CliTranscriptLane::open_at("cli-1".into(), TranscriptMode::Dictation, path.clone())
                .unwrap();

        lane.publish_draft("Dobra", &segment("Dobra", 0.0, 1.2))
            .unwrap();
        lane.publish_sealed("Dobra, powiem Ci", &[segment("Dobra, powiem Ci", 0.0, 2.5)])
            .unwrap();
        lane.publish_ended(TranscriptSessionEndReason::Completed)
            .unwrap();

        let lines = read(&path);
        assert_eq!(lines.len(), 4);
        assert_eq!(lines[0].status, "session_started");
        assert_eq!(lines[1].status, "utterance_draft");
        assert_eq!(lines[2].status, "transcript_sealed");
        assert_eq!(lines[3].status, "session_ended");
        for line in &lines {
            assert_eq!(line.source.as_deref(), Some(CLI_FILE_VERDICT_SOURCE));
        }
    }

    /// A file pass has no occurrence identity. If these ever become Some, a
    /// consumer could mistake a CLI document for ledger-anchored PCM truth.
    #[test]
    fn cli_events_never_claim_capture_sample_offsets() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("events.jsonl");
        let mut lane =
            CliTranscriptLane::open_at("cli-2".into(), TranscriptMode::Dictation, path.clone())
                .unwrap();
        lane.publish_sealed("tekst", &[segment("tekst", 1.0, 2.0)])
            .unwrap();

        let sealed = read(&path)
            .into_iter()
            .find(|event| event.status == "transcript_sealed")
            .expect("sealed event");
        assert!(sealed.sample_start.is_none());
        assert!(sealed.sample_end.is_none());
        assert!(sealed.capture_epoch.is_none());
        assert!(sealed.coverage.is_none());
        assert_eq!(sealed.audio_start_seconds, Some(1.0));
        assert_eq!(sealed.audio_end_seconds, Some(2.0));
    }

    /// The witness here is the EFFECT a tailer sees, not the field name.
    ///
    /// A cumulative lane also writes three events called `utterance_draft` with
    /// three non-null `utterance_id`s, so asserting those names would pass on
    /// the broken behaviour. What separates the two is whether draft N repeats
    /// draft N-1's words — an append-only reader printing each draft is exactly
    /// what breaks when it does.
    #[test]
    fn each_draft_carries_its_own_utterance_not_the_growing_document() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("events.jsonl");
        let mut lane =
            CliTranscriptLane::open_at("cli-4".into(), TranscriptMode::Dictation, path.clone())
                .unwrap();

        let spoken = ["pierwsza fraza", "druga fraza", "trzecia fraza"];
        let decoded: Vec<TranscriptSegment> = spoken
            .iter()
            .enumerate()
            .map(|(index, text)| {
                let start = index as f32;
                segment(text, start, start + 1.0)
            })
            .chain(std::iter::once(segment("   ", 9.0, 9.5)))
            .collect();

        // Drive the same entry point the CLI drives — the grain decision is
        // inside it, so a cumulative regression cannot hide behind a call site.
        let printed = lane.publish_segments(&decoded).unwrap();
        assert_eq!(printed, spoken, "stdout lines must be the utterances too");
        lane.publish_sealed(&spoken.join(" "), &[segment("all", 0.0, 3.0)])
            .unwrap();

        let drafts: Vec<CleanTranscriptEvent> = read(&path)
            .into_iter()
            .filter(|event| event.status == "utterance_draft")
            .collect();
        assert_eq!(drafts.len(), spoken.len());

        // The effect: each draft is exactly its own utterance, and no draft
        // swallows the one before it.
        for (draft, expected) in drafts.iter().zip(spoken.iter()) {
            assert_eq!(&draft.text, expected);
        }
        for pair in drafts.windows(2) {
            assert!(
                !pair[1].text.contains(&pair[0].text),
                "draft {:?} repeats the previous draft {:?} — a tailer would reprint it",
                pair[1].text,
                pair[0].text
            );
        }

        // Utterance numbering is its own axis: 1..n, and it does NOT track the
        // line sequence, which also counts `session_started`.
        let ids: Vec<Option<u64>> = drafts.iter().map(|draft| draft.utterance_id).collect();
        assert_eq!(ids, vec![Some(1), Some(2), Some(3)]);
        assert_ne!(
            drafts[0].sequence,
            drafts[0].utterance_id.unwrap(),
            "utterance_id must not be an alias for the bus sequence"
        );

        // The document lives on the seal, and only there.
        let sealed = read(&path)
            .into_iter()
            .find(|event| event.status == "transcript_sealed")
            .expect("sealed event");
        assert_eq!(sealed.text, spoken.join(" "));
    }

    #[test]
    fn file_verdict_keeps_its_own_session_wav_never_last_session_identity() {
        let temp = tempfile::tempdir().unwrap();
        let sessions = temp.path().join("sessions");
        let source = temp.path().join("last_session.wav");
        std::fs::write(&source, b"pcm-bytes").unwrap();
        let bus = temp.path().join("events.jsonl");
        let lane = CliTranscriptLane::open_at("cli-wav-01".into(), TranscriptMode::Dictation, bus)
            .unwrap();

        let dest = lane.retain_source_wav_at(&source, &sessions).unwrap();
        assert_eq!(dest, sessions.join("cli-wav-01.wav"));
        assert_eq!(dest.file_name().unwrap(), "cli-wav-01.wav");
        assert_ne!(dest.file_name().unwrap(), "last_session.wav");
        assert_eq!(std::fs::read(&dest).unwrap(), b"pcm-bytes");
        assert!(!sessions.join("last_session.wav").exists());
    }

    /// Same rule the app's bus holds: no terminal line for a session that never
    /// opened, so an unfinished run is visible as a start without an end.
    #[test]
    fn an_unstarted_lane_writes_nothing_at_all() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("events.jsonl");
        let mut lane =
            CliTranscriptLane::open_at("cli-3".into(), TranscriptMode::Dictation, path.clone())
                .unwrap();
        lane.publish_ended(TranscriptSessionEndReason::Completed)
            .unwrap();
        assert!(!path.exists() || std::fs::read_to_string(&path).unwrap().is_empty());
    }
}
