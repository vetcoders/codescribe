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
use codescribe_core::pipeline::acoustic_ledger::{AcousticLedger, AcousticSerial};
use codescribe_core::pipeline::contracts::{AcousticSpanGrain, TranscriptSegment};
use serde::{Deserialize, Serialize};

use super::emitter::{ReducerAction, TranscriptRevision};

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

#[cfg(test)]
#[path = "../../tests/support/p0_b_five_iwo.rs"]
mod p0_b_five_iwo;

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
    Phrase,
    Utterance,
}

fn is_word_grain(grain: &TranscriptWordGrain) -> bool {
    matches!(grain, TranscriptWordGrain::Word)
}

/// One span on the capture PCM clock: text + samples + intensity.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TranscriptWordSpan {
    pub text: String,
    pub session_id: String,
    pub capture_epoch: u64,
    pub sample_start: u64,
    pub sample_end: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub energy_db: Option<f32>,
    #[serde(default, skip_serializing_if = "is_word_grain")]
    pub grain: TranscriptWordGrain,
}

/// Falsifiable coverage result for one transcript event. Failed receipts keep
/// the clean reducer bytes visible while refusing to pretend they are anchored.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TranscriptCoverageReceipt {
    pub passed: bool,
    pub code: String,
}

/// Lossless observer projection of one ledger-owned acoustic receipt chain.
/// The Bus copies these values; it never re-reads energy, admits evidence,
/// chooses a label, or decides whether an occurrence is sealed.
///
/// W2 input: the ledger receipt bundle attached to one reducer entry. The
/// canonical receipt encodings remain opaque here so their decision history
/// cannot be rewritten by the projection layer.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProjectedAcousticReceipt {
    pub acoustic_serial_version: u16,
    pub acoustic_serial: String,
    pub session_id: String,
    pub capture_epoch: u64,
    pub sample_start: u64,
    pub sample_end: u64,
    pub duration_ms: u64,
    pub energy_integral: f64,
    pub mean_rms_dbfs: f32,
    pub peak_dbfs: f32,
    pub vad_open_sample: u64,
    pub vad_close_sample: u64,
    pub evidence_calibration_version: String,
    /// Canonical, immutable encodings minted by the acoustic ledger.
    pub word_evidence_receipts: Vec<String>,
    /// Complete Apple/Whisper/text-layer candidate and decision history.
    pub layer_decision_receipts: Vec<String>,
    pub seal_receipt: Option<String>,
    pub manual_edit_receipt: Option<String>,
}

/// Append-only Bus observation of one reducer revision entry. Every truth
/// field is supplied by the reducer/ledger; sequence and emission time are the
/// only Bus-owned metadata.
///
/// W2 input: a reducer revision. W2 output: the recording bridge projection
/// event. Emission and bridge conversion are intentionally unresolved in W1.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TranscriptBusEvidenceEvent {
    pub schema: String,
    pub sequence: u64,
    pub emitted_at: String,
    pub session_id: String,
    pub mode: TranscriptMode,
    pub reducer_revision: u64,
    pub reducer_action: String,
    pub occurrence_session_id: String,
    pub capture_epoch: u64,
    pub sample_start: u64,
    pub sample_end: u64,
    pub document_index: u64,
    pub label: String,
    pub rendered_text: String,
    pub acoustic_receipts: Vec<ProjectedAcousticReceipt>,
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
    pub capture_epoch: Option<u64>,
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
    pub coverage: Option<TranscriptCoverageReceipt>,
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
    energy_lookup: fn(u64, u64) -> Option<f32>,
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
    fn project_serial(
        serial: &AcousticSerial,
        word_evidence_receipts: Vec<String>,
        layer_decision_receipts: Vec<String>,
        seal_receipt: Option<String>,
        manual_edit_receipt: Option<String>,
    ) -> ProjectedAcousticReceipt {
        ProjectedAcousticReceipt {
            acoustic_serial_version: serial.version,
            acoustic_serial: serial.digest.clone(),
            session_id: serial.occurrence.session.clone(),
            capture_epoch: serial.occurrence.capture_epoch,
            sample_start: serial.occurrence.sample_start,
            sample_end: serial.occurrence.sample_end,
            duration_ms: serial.duration_ms.max(0.0) as u64,
            energy_integral: serial.energy_integral,
            mean_rms_dbfs: serial.mean_rms_dbfs as f32,
            peak_dbfs: serial.peak_dbfs as f32,
            vad_open_sample: serial.vad_open_sample.unwrap_or(serial.occurrence.sample_start),
            vad_close_sample: serial.vad_close_sample.unwrap_or(serial.occurrence.sample_end),
            evidence_calibration_version: serial.evidence_calibration_version.clone(),
            word_evidence_receipts,
            layer_decision_receipts,
            seal_receipt,
            manual_edit_receipt,
        }
    }

    /// Observe one reducer revision and copy its ledger receipts byte-for-byte.
    /// The Bus owns only append sequence and emission time; it cannot admit,
    /// reduce, choose labels, or infer finality.
    pub fn publish_revision(
        &self,
        revision: &TranscriptRevision,
        ledger: &AcousticLedger,
    ) -> Vec<TranscriptBusEvidenceEvent> {
        let reducer_action = match &revision.action {
            ReducerAction::ApplyLedgerDecision { .. } => "apply_ledger_decision",
            ReducerAction::RecordLedgerSeal { terminal: true, .. } => {
                "record_ledger_terminal_seal"
            }
            ReducerAction::RecordLedgerSeal { terminal: false, .. } => "record_ledger_seal",
            ReducerAction::ApplyManualEdit { .. } => "apply_manual_edit",
        };
        let mut writer = self
            .writer
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if writer.sealed && !matches!(&revision.action, ReducerAction::ApplyManualEdit { .. }) {
            return Vec::new();
        }
        let mut emitted = Vec::new();
        for (document_index, entry) in revision.entries.iter().enumerate() {
            let Some(serial) = ledger.serial_of(&entry.occurrence) else {
                continue;
            };
            let event = TranscriptBusEvidenceEvent {
                schema: "codescribe.transcript-evidence.v1".to_string(),
                sequence: writer.sequence.saturating_add(1),
                emitted_at: Utc::now().to_rfc3339_opts(SecondsFormat::Micros, true),
                session_id: self.session.session_id.clone(),
                mode: self.session.mode,
                reducer_revision: revision.revision,
                reducer_action: reducer_action.to_string(),
                occurrence_session_id: entry.occurrence.session.clone(),
                capture_epoch: entry.occurrence.capture_epoch,
                sample_start: entry.occurrence.sample_start,
                sample_end: entry.occurrence.sample_end,
                document_index: document_index as u64,
                label: entry.label.clone(),
                rendered_text: revision.rendered_text.clone(),
                acoustic_receipts: vec![Self::project_serial(
                    serial,
                    entry.word_evidence_receipts.clone(),
                    entry.layer_decision_receipts.clone(),
                    entry.seal_receipt.clone(),
                    entry.manual_edit_receipt.clone(),
                )],
            };
            if let Err(error) = self.write_evidence_event_locked(&mut writer, &event) {
                self.log_write_error(error);
                break;
            }
            emitted.push(event);
        }
        if matches!(
            &revision.action,
            ReducerAction::RecordLedgerSeal { terminal: true, .. }
        ) {
            writer.sealed = true;
        }
        emitted
    }

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
        Self::open_at_with_energy(session, path, sample_rate_override, session_energy_db)
    }

    fn open_at_with_energy(
        session: TranscriptSession,
        path: PathBuf,
        sample_rate_override: Option<u32>,
        energy_lookup: fn(u64, u64) -> Option<f32>,
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
            energy_lookup,
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
        let (words, coverage) = word_spans_from_draft(&utterance, self.energy_lookup);
        let identity = utterance.acoustic.as_ref().map(|value| &value.range);
        let event = CleanTranscriptEvent {
            schema: "codescribe.transcript.v1".to_string(),
            sequence: 0,
            session_id: String::new(),
            mode: self.session.mode,
            utterance_id: Some(utterance.utterance_id),
            emitted_at: String::new(),
            status: status.to_string(),
            sample_rate_hz: sample_rate,
            capture_epoch: identity.map(|range| range.capture_epoch),
            sample_start: identity.map(|range| range.sample_start),
            sample_end: identity.map(|range| range.sample_end),
            audio_start_seconds: identity
                .and_then(|range| samples_to_seconds(range.sample_start, sample_rate)),
            audio_end_seconds: identity
                .and_then(|range| samples_to_seconds(range.sample_end, sample_rate)),
            text: utterance.text.clone(),
            segments: utterance.segments.clone(),
            words,
            coverage: Some(coverage),
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
        let mut clock = aggregate_seal_clock(&writer.drafts, sample_rate, self.energy_lookup);
        let event = CleanTranscriptEvent {
            schema: "codescribe.transcript.v1".to_string(),
            sequence: 0,
            session_id: String::new(),
            mode: self.session.mode,
            utterance_id: None,
            emitted_at: String::new(),
            status: "transcript_sealed".to_string(),
            sample_rate_hz: sample_rate,
            capture_epoch: clock.capture_epoch,
            sample_start: clock.sample_start,
            sample_end: clock.sample_end,
            audio_start_seconds: clock.audio_start_seconds,
            audio_end_seconds: clock.audio_end_seconds,
            text,
            segments: clock.segments,
            words: clock.words,
            coverage: Some(clock.coverage),
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
                capture_epoch: None,
                sample_start: None,
                sample_end: None,
                audio_start_seconds: None,
                audio_end_seconds: None,
                text: String::new(),
                segments: Vec::new(),
                words: Vec::new(),
                coverage: None,
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

    fn write_evidence_event_locked(
        &self,
        writer: &mut TranscriptBusWriter,
        event: &TranscriptBusEvidenceEvent,
    ) -> io::Result<()> {
        let mut encoded = serde_json::to_vec(event).map_err(io::Error::other)?;
        encoded.push(b'\n');
        writer.file.write_all(&encoded)?;
        writer.file.flush()?;
        writer.sequence = event.sequence;
        Ok(())
    }

    fn log_write_error(&self, error: io::Error) {
        let file = self
            .path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("transcript-events.jsonl");
        tracing::warn!(%error, file, "clean transcript event write failed");
    }
}

fn lexical_signature(text: &str) -> Vec<String> {
    text.split(|character: char| !character.is_alphanumeric())
        .filter(|token| !token.is_empty())
        .map(str::to_lowercase)
        .collect()
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

fn samples_to_seconds(sample: u64, sample_rate: Option<u32>) -> Option<f32> {
    sample_rate
        .filter(|rate| *rate > 0)
        .map(|rate| sample as f32 / rate as f32)
}

fn bus_grain(grain: AcousticSpanGrain) -> TranscriptWordGrain {
    match grain {
        AcousticSpanGrain::Word => TranscriptWordGrain::Word,
        AcousticSpanGrain::Phrase => TranscriptWordGrain::Phrase,
        AcousticSpanGrain::Utterance => TranscriptWordGrain::Utterance,
    }
}

fn failed_coverage(code: &str) -> TranscriptCoverageReceipt {
    TranscriptCoverageReceipt {
        passed: false,
        code: code.to_string(),
    }
}

struct SealClock {
    capture_epoch: Option<u64>,
    sample_start: Option<u64>,
    sample_end: Option<u64>,
    audio_start_seconds: Option<f32>,
    audio_end_seconds: Option<f32>,
    segments: Vec<TranscriptSegment>,
    words: Vec<TranscriptWordSpan>,
    coverage: TranscriptCoverageReceipt,
}

fn aggregate_seal_clock(
    drafts: &BTreeMap<u64, TranscriptDraft>,
    sample_rate: Option<u32>,
    energy_lookup: fn(u64, u64) -> Option<f32>,
) -> SealClock {
    let mut sample_start = None;
    let mut sample_end = None;
    let mut capture_epoch = None;
    let mut mixed_epochs = false;
    let mut segments = Vec::new();
    let mut words = Vec::new();
    let mut coverage = TranscriptCoverageReceipt {
        passed: true,
        code: "anchored_voiced_coverage".to_string(),
    };
    for draft in drafts.values() {
        if let Some(acoustic) = &draft.acoustic {
            if !mixed_epochs {
                match capture_epoch {
                    None => capture_epoch = Some(acoustic.range.capture_epoch),
                    Some(epoch) if epoch != acoustic.range.capture_epoch => {
                        mixed_epochs = true;
                        capture_epoch = None;
                        sample_start = None;
                        sample_end = None;
                        words.clear();
                        coverage = failed_coverage("multiple_capture_epochs");
                    }
                    Some(_) => {}
                }
            }
            if !mixed_epochs && capture_epoch.is_some() {
                sample_start = Some(
                    sample_start.map_or(acoustic.range.sample_start, |seen: u64| {
                        seen.min(acoustic.range.sample_start)
                    }),
                );
                sample_end = Some(sample_end.map_or(acoustic.range.sample_end, |seen: u64| {
                    seen.max(acoustic.range.sample_end)
                }));
            }
        }
        segments.extend(draft.segments.iter().cloned());
        let (draft_words, draft_coverage) = word_spans_from_draft(draft, energy_lookup);
        if coverage.passed {
            if draft_coverage.passed {
                words.extend(draft_words);
            } else {
                // Coverage is aggregate authority: one failed draft invalidates
                // every accumulated lexical anchor for the seal. Keeping the
                // earlier words beside a failed receipt would publish a
                // plausible-looking but incomplete transcript clock.
                words.clear();
                coverage = draft_coverage;
            }
        }
    }
    if drafts.is_empty() {
        coverage = failed_coverage("missing_pcm_identity");
    }
    SealClock {
        capture_epoch,
        sample_start,
        sample_end,
        audio_start_seconds: sample_start
            .and_then(|sample| samples_to_seconds(sample, sample_rate)),
        audio_end_seconds: sample_end.and_then(|sample| samples_to_seconds(sample, sample_rate)),
        segments,
        words,
        coverage,
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

    fn voiced_energy(_start: u64, _end: u64) -> Option<f32> {
        Some(-24.0)
    }

    fn silence(_start: u64, _end: u64) -> Option<f32> {
        None
    }

    /// Conservation at the delivery seam: one published span per drafted span,
    /// in order, or none at all with a receipt naming why.
    ///
    /// Five acoustic occurrences of one name are byte-identical, so nothing
    /// downstream of this point can tell them apart again. The bus is where the
    /// count is still checkable, and it must not be the layer that loses one.
    #[test]
    fn every_drafted_acoustic_span_is_published_exactly_once() {
        let drafted: Vec<(&str, u64, u64, AcousticSpanGrain)> = (0..5)
            .map(|i| {
                (
                    "Iwo",
                    i * 16_000,
                    i * 16_000 + 8_000,
                    AcousticSpanGrain::Word,
                )
            })
            .collect();
        let draft = TranscriptDraft {
            utterance_id: 1,
            text: "Iwo Iwo Iwo Iwo Iwo".to_string(),
            start_seconds: 0.0,
            end_seconds: 5.0,
            segments: Vec::new(),
            acoustic: Some(acoustic("s", 1, 0, 80_000, drafted)),
        };
        let (spans, coverage) = word_spans_from_draft(&draft, voiced_energy);
        assert!(coverage.passed, "coverage receipt: {coverage:?}");
        assert_eq!(spans.len(), 5, "five drafted spans, five published");
        let starts: Vec<u64> = spans.iter().map(|span| span.sample_start).collect();
        assert_eq!(starts, vec![0, 16_000, 32_000, 48_000, 64_000]);
        assert!(
            spans.iter().all(|span| span.text == "Iwo"),
            "identical text is not a reason to drop a span"
        );
    }

    /// Missing voiced energy is missing evidence, not evidence of absence. The
    /// anchors are withheld with a receipt; the text stays visible and no span
    /// is silently published as though it had been verified.
    #[test]
    fn a_span_without_voiced_energy_is_withheld_whole_not_partially_published() {
        let draft = TranscriptDraft {
            utterance_id: 1,
            text: "Iwo Iwo".to_string(),
            start_seconds: 0.0,
            end_seconds: 2.0,
            segments: Vec::new(),
            acoustic: Some(acoustic(
                "s",
                1,
                0,
                32_000,
                vec![
                    ("Iwo", 0, 8_000, AcousticSpanGrain::Word),
                    ("Iwo", 16_000, 24_000, AcousticSpanGrain::Word),
                ],
            )),
        };
        let (spans, coverage) = word_spans_from_draft(&draft, silence);
        assert!(!coverage.passed);
        assert_eq!(coverage.code, "lexical_span_without_voiced_energy");
        assert!(
            spans.is_empty(),
            "all or nothing: a half-verified anchor set is worse than none"
        );
    }

    #[test]
    fn one_failed_draft_clears_all_aggregate_word_spans() {
        let covered = TranscriptDraft {
            utterance_id: 1,
            text: "pierwszy fragment".to_string(),
            start_seconds: 0.0,
            end_seconds: 1.0,
            segments: Vec::new(),
            acoustic: Some(acoustic(
                "s",
                1,
                0,
                16_000,
                vec![("pierwszy fragment", 0, 16_000, AcousticSpanGrain::Phrase)],
            )),
        };
        let uncovered = TranscriptDraft {
            utterance_id: 2,
            text: "drugi fragment".to_string(),
            start_seconds: 1.0,
            end_seconds: 2.0,
            segments: Vec::new(),
            acoustic: Some(acoustic("s", 1, 16_000, 32_000, Vec::new())),
        };
        let drafts = BTreeMap::from([(1, covered), (2, uncovered)]);

        let seal = aggregate_seal_clock(&drafts, Some(16_000), voiced_energy);

        assert!(!seal.coverage.passed);
        assert_eq!(seal.coverage.code, "missing_lexical_coverage");
        assert!(
            seal.words.is_empty(),
            "a failed aggregate receipt cannot leave earlier words publishable"
        );
    }

    /// Utterance grain travels as utterance grain. Presenting one span for a
    /// whole utterance as word grain would invent per-word PCM identities the
    /// payload never carried.
    #[test]
    fn utterance_grain_is_not_republished_as_word_grain() {
        let draft = TranscriptDraft {
            utterance_id: 1,
            text: "całe zdanie".to_string(),
            start_seconds: 0.0,
            end_seconds: 2.0,
            segments: Vec::new(),
            acoustic: Some(acoustic(
                "s",
                1,
                0,
                32_000,
                vec![("całe zdanie", 0, 32_000, AcousticSpanGrain::Utterance)],
            )),
        };
        let (spans, coverage) = word_spans_from_draft(&draft, voiced_energy);
        assert!(coverage.passed);
        assert_eq!(spans.len(), 1);
        assert_eq!(spans[0].grain, TranscriptWordGrain::Utterance);
    }

    #[test]
    fn bus_flushes_start_draft_and_seal_as_private_ndjson() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("events.jsonl");
        let bus = TranscriptBus::open_at_with_energy(
            TranscriptSession {
                session_id: "session-agent".to_string(),
                mode: TranscriptMode::Agent,
            },
            path.clone(),
            Some(48_000),
            voiced_energy,
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
                acoustic: Some(acoustic(
                    "pipeline-session",
                    4,
                    12_000,
                    72_000,
                    vec![("clean final", 12_000, 72_000, AcousticSpanGrain::Utterance)],
                )),
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
                acoustic: None,
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
        assert_eq!(lines[2].words[0].capture_epoch, 4);
        assert_eq!(lines[2].words[0].session_id, "pipeline-session");
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
        let bus = TranscriptBus::open_at_with_energy(
            TranscriptSession {
                session_id: "session-words".to_string(),
                mode: TranscriptMode::Dictation,
            },
            path.clone(),
            Some(16_000),
            voiced_energy,
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
                acoustic: Some(acoustic(
                    "pipeline-words",
                    9,
                    16_000,
                    32_000,
                    vec![
                        ("dwa", 16_000, 22_400, AcousticSpanGrain::Word),
                        ("slowa", 22_400, 32_000, AcousticSpanGrain::Word),
                    ],
                )),
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
        assert_eq!(seal.coverage.as_ref().map(|value| value.passed), Some(true));
    }

    #[test]
    fn silence_and_missing_identity_cannot_gain_published_lexical_spans() {
        let draft = TranscriptDraft {
            utterance_id: 1,
            text: "modelki trzy".to_string(),
            start_seconds: 0.0,
            end_seconds: 1.0,
            segments: Vec::new(),
            acoustic: Some(acoustic(
                "silence",
                1,
                0,
                16_000,
                vec![("modelki trzy", 0, 16_000, AcousticSpanGrain::Phrase)],
            )),
        };
        let (words, receipt) = word_spans_from_draft(&draft, silence);
        assert!(words.is_empty(), "energy absence cannot mint lexical spans");
        assert!(!receipt.passed);
        assert_eq!(receipt.code, "lexical_span_without_voiced_energy");

        let mut missing = draft;
        missing.acoustic = None;
        let (words, receipt) = word_spans_from_draft(&missing, voiced_energy);
        assert!(words.is_empty());
        assert_eq!(receipt.code, "missing_pcm_identity");
    }

    #[test]
    #[test]
    fn five_acoustic_iwo_survive_reducer_and_transcript_bus() {
        use super::super::emitter::reduce_transcript_events;
        use codescribe_core::pipeline::contracts::EngineEvent;

        let spans = (0..5u64)
            .map(|i| ("Iwo", i * 1_600, i * 1_600 + 1_600, AcousticSpanGrain::Word))
            .collect();
        let identity = acoustic("take", 1, 0, 8_000, spans);
        let event = EngineEvent::UtteranceFinal {
            utterance_id: 1,
            text: "Iwo Iwo Iwo Iwo Iwo".into(),
            raw_text: "Iwo Iwo Iwo Iwo Iwo".into(),
            start_ts: 0.0,
            end_ts: 0.5,
            segments: Vec::new(),
            vad_speech_pct: None,
            avg_logprob: None,
            compression_ratio: None,
            quality_gate_dropped: false,
            confidence_flags: Vec::new(),
            acoustic: Some(identity.clone()),
        };
        let reducer = reduce_transcript_events(&[event]);
        let delivered = reducer
            .rendered_text()
            .split_whitespace()
            .filter(|word| word.eq_ignore_ascii_case("iwo"))
            .count();
        assert_eq!(
            delivered,
            5,
            "reducer delivery: {}",
            reducer.rendered_text()
        );

        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("events.jsonl");
        let bus = TranscriptBus::open_at_with_energy(
            TranscriptSession {
                session_id: "iwo-five".to_string(),
                mode: TranscriptMode::Dictation,
            },
            path.clone(),
            Some(16_000),
            voiced_energy,
        )
        .unwrap();
        bus.publish_started();
        bus.publish_draft(
            TranscriptDraftStatus::Created,
            TranscriptDraft {
                utterance_id: 1,
                text: reducer.rendered_text(),
                start_seconds: 0.0,
                end_seconds: 0.5,
                segments: Vec::new(),
                acoustic: Some(identity),
            },
        );
        bus.publish_sealed(reducer.rendered_text(), None);
        let raw = std::fs::read_to_string(&path).unwrap();
        let seal: CleanTranscriptEvent = raw
            .lines()
            .map(|line| serde_json::from_str::<CleanTranscriptEvent>(line).unwrap())
            .find(|event| event.status == "transcript_sealed")
            .expect("seal");
        assert_eq!(seal.words.len(), 5);
        assert!(seal.words.iter().all(|word| word.text == "Iwo"));
        assert_eq!(
            seal.text
                .split_whitespace()
                .filter(|word| word.eq_ignore_ascii_case("iwo"))
                .count(),
            5
        );
    }
}
