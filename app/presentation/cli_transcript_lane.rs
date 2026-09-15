//! CLI file-transcription lane on the clean transcript bus.
//!
//! The file engine owns document assembly. Rows keep per-segment `text` for
//! utterance consumers and a complete `rendered_text` for passive canvases.
//! `source=cli_file_verdict` identifies file output without claiming ledger receipts.

use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

use chrono::{SecondsFormat, Utc};
use codescribe_core::pipeline::contracts::TranscriptSegment;
use sha2::{Digest, Sha256};
use uuid::Uuid;

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
    document: String,
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
            document: String::new(),
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

    /// Publish a segment and its engine-assembled document snapshot.
    pub fn publish_draft(&mut self, text: &str, segment: &TranscriptSegment) -> io::Result<()> {
        self.publish_started()?;
        self.utterance_counter = self.utterance_counter.saturating_add(1);
        let mut event = self.lifecycle("utterance_draft", Some(self.utterance_counter));
        event.text = text.to_string();
        if !self.document.is_empty() {
            self.document.push(' ');
        }
        self.document.push_str(text);
        event.can_copy = !self.document.is_empty();
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

    /// Publish and return printable segments in decoder order.
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
        self.document = text.to_string();
        event.phase = super::transcript_bus::TranscriptProjectionPhase::Formatted;
        event.can_copy = !text.is_empty();
        event.segments = segments.to_vec();
        event.audio_start_seconds = segments.first().map(|segment| segment.start_ts);
        event.audio_end_seconds = segments.last().map(|segment| segment.end_ts);
        self.write(event)
    }

    /// Retain this run's audio as `sessions/<session_id>.wav`.
    ///
    /// `RIFF....WAVE` sources keep their bytes. Any other container is decoded
    /// with `load_audio_file` and written as PCM-16 WAV. Identical retained
    /// bytes share one inode via `sessions/.index/<sha256>` and a hard link
    /// (copy if linking is refused). Demux identity stays that session path.
    /// `last_session.wav` is a latest-app-take alias and is never written here.
    pub fn retain_source_wav(&self, source: &Path) -> io::Result<PathBuf> {
        self.retain_source_wav_at(
            source,
            &codescribe_core::config::Config::config_dir().join("sessions"),
        )
    }

    pub fn retain_source_wav_at(&self, source: &Path, sessions_dir: &Path) -> io::Result<PathBuf> {
        let id = self.session_id.as_str();
        if !is_safe_session_stem(id) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "session id is not a safe wav filename",
            ));
        }
        let meta = fs::metadata(source)?;
        if !meta.is_file() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "CLI source wav must be a regular file",
            ));
        }
        fs::create_dir_all(sessions_dir)?;
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

        let owned_temp = if sniff_riff_wave(source)? {
            None
        } else {
            let tmp = retain_temp_path(sessions_dir);
            if let Err(err) = decode_container_to_wav(source, &tmp) {
                let _ = fs::remove_file(&tmp);
                return Err(err);
            }
            Some(tmp)
        };
        let retained_is_owned_temp = owned_temp.is_some();
        let installed = {
            let retained = owned_temp.as_deref().unwrap_or(source);
            install_retained_identity(id, retained, &dest, sessions_dir, retained_is_owned_temp)
        };
        match installed {
            Ok(consumed_owned_temp) => {
                if let Some(tmp) = owned_temp
                    && !consumed_owned_temp
                {
                    let _ = fs::remove_file(tmp);
                }
                Ok(dest)
            }
            Err(err) => {
                if let Some(tmp) = owned_temp {
                    let _ = fs::remove_file(tmp);
                }
                Err(err)
            }
        }
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
        event.terminal = true;
        event.phase = if reason == TranscriptSessionEndReason::Completed {
            super::transcript_bus::TranscriptProjectionPhase::Formatted
        } else {
            super::transcript_bus::TranscriptProjectionPhase::Error
        };
        event.can_copy = !self.document.is_empty();
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
            phase: super::transcript_bus::TranscriptProjectionPhase::Listening,
            can_paste: false,
            can_insert: false,
            can_copy: false,
            can_retranscribe: false,
            can_format: false,
            can_send_to_agent: false,
            terminal: false,
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

        #[derive(serde::Serialize)]
        struct DocumentRow<'a> {
            #[serde(flatten)]
            event: &'a CleanTranscriptEvent,
            #[serde(skip_serializing_if = "Option::is_none")]
            rendered_text: Option<&'a str>,
        }
        let rendered_text = matches!(
            event.status.as_str(),
            "utterance_draft" | "transcript_sealed"
        )
        .then_some(self.document.as_str());
        let mut encoded = serde_json::to_vec(&DocumentRow {
            event: &event,
            rendered_text,
        })
        .map_err(io::Error::other)?;
        encoded.push(b'\n');
        file.write_all(&encoded)?;
        file.flush()?;
        self.sequence = next;
        Ok(())
    }
}

const HASH_BUF_BYTES: usize = 8 * 1024 * 1024;

fn is_safe_session_stem(id: &str) -> bool {
    (8..=80).contains(&id.len())
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

fn retain_temp_path(sessions_dir: &Path) -> PathBuf {
    sessions_dir.join(format!(".retain-{}.tmp", Uuid::new_v4()))
}

fn sniff_riff_wave(path: &Path) -> io::Result<bool> {
    let mut file = File::open(path)?;
    let mut header = [0u8; 12];
    let read = file.read(&mut header)?;
    Ok(read == 12 && header.starts_with(b"RIFF") && &header[8..12] == b"WAVE")
}

fn sha256_file(path: &Path) -> io::Result<String> {
    let mut file = File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; HASH_BUF_BYTES];
    loop {
        let n = file.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(format!("{:x}", hasher.finalize()))
}

fn parse_canonical_wav_name(name: &str) -> Option<&str> {
    if name.contains('/') || name.contains('\\') || name.contains('\0') {
        return None;
    }
    if name == "last_session.wav" || name.starts_with('.') {
        return None;
    }
    let stem = name.strip_suffix(".wav")?;
    if !is_safe_session_stem(stem) {
        return None;
    }
    Some(name)
}

fn read_live_canonical(index_path: &Path, sessions_dir: &Path) -> io::Result<Option<PathBuf>> {
    let text = match fs::read_to_string(index_path) {
        Ok(text) => text,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(err) => return Err(err),
    };
    let Some(name) = parse_canonical_wav_name(text.trim()) else {
        return Ok(None);
    };
    let canonical = sessions_dir.join(name);
    match fs::symlink_metadata(&canonical) {
        Ok(meta) if meta.is_file() => Ok(Some(canonical)),
        Ok(_) => Ok(None),
        Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(err) => Err(err),
    }
}

fn write_index_atomic(index_dir: &Path, digest: &str, canonical_name: &str) -> io::Result<()> {
    let dest = index_dir.join(digest);
    let tmp = index_dir.join(format!(".retain-{}.tmp", Uuid::new_v4()));
    if let Err(err) = fs::write(&tmp, format!("{canonical_name}\n")) {
        let _ = fs::remove_file(&tmp);
        return Err(err);
    }
    fs::rename(&tmp, dest).inspect_err(|_| {
        let _ = fs::remove_file(&tmp);
    })
}

fn is_hard_link_fallback(err: &io::Error) -> bool {
    matches!(
        err.kind(),
        io::ErrorKind::AlreadyExists
            | io::ErrorKind::PermissionDenied
            | io::ErrorKind::CrossesDevices
    ) || matches!(
        err.raw_os_error(),
        Some(libc::EXDEV | libc::EPERM | libc::EEXIST | libc::EACCES)
    )
}

fn same_regular_inode(left: &Path, right: &Path) -> io::Result<bool> {
    let right_meta = match fs::symlink_metadata(right) {
        Ok(meta) => meta,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(false),
        Err(err) => return Err(err),
    };
    if right_meta.file_type().is_symlink() || !right_meta.is_file() {
        return Ok(false);
    }
    let left_meta = fs::symlink_metadata(left)?;
    if left_meta.file_type().is_symlink() || !left_meta.is_file() {
        return Ok(false);
    }
    Ok(left_meta.dev() == right_meta.dev() && left_meta.ino() == right_meta.ino())
}

fn publish_retained_path(
    from: &Path,
    dest: &Path,
    sessions_dir: &Path,
    try_link: bool,
) -> io::Result<()> {
    if same_regular_inode(from, dest)? {
        return Ok(());
    }
    let tmp = retain_temp_path(sessions_dir);
    let prepared = if try_link {
        // nosemgrep: rust.actix.path-traversal.tainted-path.tainted-path -- tmp is sessions_dir + minted `.retain-<uuid>.tmp`; from is a validated sessions leaf.
        match fs::hard_link(from, &tmp) {
            Ok(()) => Ok(()),
            Err(err) if is_hard_link_fallback(&err) => {
                let _ = fs::remove_file(&tmp);
                copy_to_minted_temp(from, &tmp)
            }
            Err(err) => {
                let _ = fs::remove_file(&tmp);
                Err(err)
            }
        }
    } else {
        copy_to_minted_temp(from, &tmp)
    };
    if let Err(err) = prepared {
        let _ = fs::remove_file(&tmp);
        return Err(err);
    }
    fs::rename(&tmp, dest).inspect_err(|_| {
        let _ = fs::remove_file(&tmp);
    })
}

fn install_retained_identity(
    session_id: &str,
    retained: &Path,
    dest: &Path,
    sessions_dir: &Path,
    retained_is_owned_temp: bool,
) -> io::Result<bool> {
    let digest = sha256_file(retained)?;
    let index_dir = sessions_dir.join(".index");
    fs::create_dir_all(&index_dir)?;
    let index_path = index_dir.join(&digest);
    if let Some(canonical) = read_live_canonical(&index_path, sessions_dir)? {
        publish_retained_path(&canonical, dest, sessions_dir, true)?;
        return Ok(false);
    }
    let consumed_owned_temp = if retained_is_owned_temp {
        fs::rename(retained, dest)?;
        true
    } else {
        publish_retained_path(retained, dest, sessions_dir, false)?;
        false
    };
    write_index_atomic(&index_dir, &digest, &format!("{session_id}.wav"))?;
    Ok(consumed_owned_temp)
}

fn copy_to_minted_temp(from: &Path, tmp: &Path) -> io::Result<()> {
    // tmp is sessions_dir + minted `.retain-<uuid>.tmp`; `from` is either the
    // metadata-checked source or a validated sessions leaf. Destination identity
    // is never a user-controlled path.
    // nosemgrep: rust.actix.path-traversal.tainted-path.tainted-path -- dest is sessions_dir + minted temp or safe session id; source metadata-checked as a regular file.
    fs::copy(from, tmp).map(|_| ())
}

fn decode_container_to_wav(source: &Path, dest: &Path) -> io::Result<()> {
    let (samples, sample_rate) =
        codescribe_core::audio::load_audio_file(source).map_err(io::Error::other)?;
    let written = write_pcm16_wav(dest, &samples, sample_rate);
    drop(samples);
    written
}

fn write_pcm16_wav(path: &Path, samples: &[f32], sample_rate: u32) -> io::Result<()> {
    if sample_rate == 0 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "decoded sample rate is 0",
        ));
    }
    let data_bytes = samples
        .len()
        .checked_mul(2)
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "decoded wav is too large"))?;
    let data_size = u32::try_from(data_bytes)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "decoded wav is too large"))?;
    let riff_size = 36u32
        .checked_add(data_size)
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "decoded wav is too large"))?;
    let file = OpenOptions::new().write(true).create_new(true).open(path)?;
    let mut writer = io::BufWriter::new(file);
    writer.write_all(b"RIFF")?;
    writer.write_all(&riff_size.to_le_bytes())?;
    writer.write_all(b"WAVE")?;
    writer.write_all(b"fmt ")?;
    writer.write_all(&16u32.to_le_bytes())?;
    writer.write_all(&1u16.to_le_bytes())?;
    writer.write_all(&1u16.to_le_bytes())?;
    writer.write_all(&sample_rate.to_le_bytes())?;
    writer.write_all(&sample_rate.saturating_mul(2).to_le_bytes())?;
    writer.write_all(&2u16.to_le_bytes())?;
    writer.write_all(&16u16.to_le_bytes())?;
    writer.write_all(b"data")?;
    writer.write_all(&data_size.to_le_bytes())?;
    for &sample in samples {
        let scaled = (sample.clamp(-1.0, 1.0) * f32::from(i16::MAX)) as i16;
        writer.write_all(&scaled.to_le_bytes())?;
    }
    writer.flush()?;
    writer
        .into_inner()
        .map_err(|err| err.into_error())?
        .sync_all()
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
    fn passive_reader_gets_complete_documents_and_preserves_the_terminal_text() {
        use super::super::transcript_projection::TranscriptProjectionReader;
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("events.jsonl");
        let mut lane =
            CliTranscriptLane::open_at("cli-test".into(), TranscriptMode::Dictation, path.clone())
                .unwrap();
        lane.publish_segments(&[segment("raz", 0.0, 1.0), segment("raz", 1.0, 2.0)])
            .unwrap();
        let verdict = "  raz raz\ne\u{301} 👩‍💻  ";
        lane.publish_sealed(verdict, &[]).unwrap();
        lane.publish_ended(TranscriptSessionEndReason::Completed)
            .unwrap();
        let mut reader = TranscriptProjectionReader::new();
        let rows: Vec<_> = std::fs::read(path)
            .unwrap()
            .chunks(7)
            .flat_map(|bytes| reader.push_bytes(bytes))
            .map(Result::unwrap)
            .collect();
        assert_eq!(
            rows.iter()
                .map(|row| row.rendered_text.as_str())
                .collect::<Vec<_>>(),
            ["raz", "raz raz", verdict, verdict]
        );
        assert!(
            rows.iter()
                .all(|row| row.source.as_deref() == Some(CLI_FILE_VERDICT_SOURCE)
                    && row.occurrence_session_id.is_empty()
                    && row.can_copy)
        );
        assert!(rows.last().unwrap().terminal);
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
        std::fs::write(&source, tiny_pcm16_wav(16_000, &[0, 1, -1])).unwrap();
        let bus = temp.path().join("events.jsonl");
        let lane = CliTranscriptLane::open_at("cli-wav-01".into(), TranscriptMode::Dictation, bus)
            .unwrap();

        let dest = lane.retain_source_wav_at(&source, &sessions).unwrap();
        assert_eq!(dest, sessions.join("cli-wav-01.wav"));
        assert_eq!(dest.file_name().unwrap(), "cli-wav-01.wav");
        assert_ne!(dest.file_name().unwrap(), "last_session.wav");
        assert_eq!(
            std::fs::read(&dest).unwrap(),
            std::fs::read(&source).unwrap()
        );
        assert!(!sessions.join("last_session.wav").exists());
    }

    fn tiny_pcm16_wav(sample_rate: u32, samples: &[i16]) -> Vec<u8> {
        let data_size = u32::try_from(samples.len() * 2).expect("fixture");
        let riff_size = 36 + data_size;
        let mut bytes = Vec::new();
        bytes.extend_from_slice(b"RIFF");
        bytes.extend_from_slice(&riff_size.to_le_bytes());
        bytes.extend_from_slice(b"WAVE");
        bytes.extend_from_slice(b"fmt ");
        bytes.extend_from_slice(&16u32.to_le_bytes());
        bytes.extend_from_slice(&1u16.to_le_bytes());
        bytes.extend_from_slice(&1u16.to_le_bytes());
        bytes.extend_from_slice(&sample_rate.to_le_bytes());
        bytes.extend_from_slice(&(sample_rate * 2).to_le_bytes());
        bytes.extend_from_slice(&2u16.to_le_bytes());
        bytes.extend_from_slice(&16u16.to_le_bytes());
        bytes.extend_from_slice(b"data");
        bytes.extend_from_slice(&data_size.to_le_bytes());
        for sample in samples {
            bytes.extend_from_slice(&sample.to_le_bytes());
        }
        bytes
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

    #[test]
    fn failed_stream_closes_once_without_sealing_partial_text() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("events.jsonl");
        let mut lane = CliTranscriptLane::open_at(
            "cli-failure".into(),
            TranscriptMode::Dictation,
            path.clone(),
        )
        .unwrap();
        lane.publish_draft("partial", &segment("partial", 0.0, 1.0))
            .unwrap();
        lane.publish_ended(TranscriptSessionEndReason::TranscriptionFailed)
            .unwrap();
        lane.publish_ended(TranscriptSessionEndReason::Completed)
            .unwrap();
        let events = read(&path);
        assert_eq!(events.len(), 3);
        assert!(
            !events
                .iter()
                .any(|event| event.status == "transcript_sealed")
        );
        let end = events.last().unwrap();
        assert_eq!(end.status, "session_ended");
        assert!(end.terminal);
        assert_eq!(
            end.phase,
            super::super::transcript_bus::TranscriptProjectionPhase::Error
        );
        assert_eq!(
            end.end_reason,
            Some(TranscriptSessionEndReason::TranscriptionFailed)
        );
        assert!(end.text.is_empty());
    }
}
