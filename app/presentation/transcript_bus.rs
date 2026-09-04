//! Durable clean transcript events for operator and control-plane consumers.
//!
//! The bus observes only occurrence-authenticated revisions emitted by the
//! [`PresentationEmitter`]. It never opens audio, accepts arbitrary text,
//! re-transcribes a file, or reconstructs text from UI deltas.

use std::fs::{File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use chrono::{SecondsFormat, Utc};
use codescribe_core::pipeline::acoustic_ledger::{
    AcousticLedger, AcousticSerial, SealCoverageReceipt, TranscriptComparisonReceipt,
};
use codescribe_core::pipeline::contracts::TranscriptSegment;
use serde::{Deserialize, Serialize};

use super::emitter::{ReducerAction, TranscriptRevision};
use crate::controller::{
    TranscriptProjectionAvailability, resolve_transcript_projection_availability,
};

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

/// Canvas phase carried by the one reducer-owned projection contract.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TranscriptProjectionPhase {
    #[default]
    Listening,
    Finalizing,
    Formatted,
    NoSpeech,
    Error,
}

impl TranscriptProjectionPhase {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Listening => "listening",
            Self::Finalizing => "finalizing",
            Self::Formatted => "formatted",
            Self::NoSpeech => "no_speech",
            Self::Error => "error",
        }
    }
}

#[cfg(test)]
#[path = "../../tests/support/p0_b_five_iwo.rs"]
mod p0_b_five_iwo;

/// Immutable identity supplied by the controller before capture starts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TranscriptSession {
    pub session_id: String,
    pub mode: TranscriptMode,
    /// A delivery target captured before the overlay can steal focus.
    pub has_latched_target: bool,
    /// Whether the explicit target is the overlay canvas itself. Normal
    /// capture-time sessions set this false because that caret fact exists
    /// only at the later defer click.
    pub latched_target_is_self: bool,
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

/// One uncovered speech span on the canonical capture PCM clock.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProjectedSealCoverageRange {
    pub sample_start: u64,
    pub sample_end: u64,
}

/// Additive coverage evidence attached to `codescribe.transcript-evidence.v1`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProjectedSealCoverageReceipt {
    pub status: String,
    pub speech_samples: u64,
    pub covered_samples: u64,
    pub uncovered_speech_ranges: Vec<ProjectedSealCoverageRange>,
    pub max_uncovered_samples: u64,
    pub incomplete_threshold_samples: u64,
    pub coverage_ratio: f64,
}

impl From<&SealCoverageReceipt> for ProjectedSealCoverageReceipt {
    fn from(receipt: &SealCoverageReceipt) -> Self {
        Self {
            status: receipt.status.as_str().to_string(),
            speech_samples: receipt.speech_samples,
            covered_samples: receipt.covered_samples,
            uncovered_speech_ranges: receipt
                .uncovered_speech_ranges
                .iter()
                .map(|range| ProjectedSealCoverageRange {
                    sample_start: range.sample_start,
                    sample_end: range.sample_end,
                })
                .collect(),
            max_uncovered_samples: receipt.max_uncovered_samples,
            incomplete_threshold_samples: receipt.incomplete_threshold_samples,
            coverage_ratio: receipt.coverage_ratio(),
        }
    }
}

/// Whole-session Apple-lane/final-pass evidence. Neither rendered string is a
/// Bus mutation input; both are retained so divergence is self-diagnosing.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProjectedTranscriptComparisonReceipt {
    pub apple_sha256: String,
    pub apple_char_count: u64,
    pub apple_rendered_text: String,
    pub final_pass_sha256: String,
    pub final_pass_char_count: u64,
    pub final_pass_rendered_text: String,
}

impl From<&TranscriptComparisonReceipt> for ProjectedTranscriptComparisonReceipt {
    fn from(receipt: &TranscriptComparisonReceipt) -> Self {
        Self {
            apple_sha256: receipt.apple_sha256.clone(),
            apple_char_count: receipt.apple_char_count,
            apple_rendered_text: receipt.apple_rendered_text.clone(),
            final_pass_sha256: receipt.final_pass_sha256.clone(),
            final_pass_char_count: receipt.final_pass_char_count,
            final_pass_rendered_text: receipt.final_pass_rendered_text.clone(),
        }
    }
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
    pub acoustic_receipts: Vec<ProjectedAcousticReceipt>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seal_coverage: Option<ProjectedSealCoverageReceipt>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub comparison: Option<ProjectedTranscriptComparisonReceipt>,
}

/// Why the controller left a Bus session. Typed on purpose: the terminal
/// line carries a reason an observer can branch on, never free text.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TranscriptSessionEndReason {
    /// The take went through the serialized stop path (sealed or zero-seal).
    Completed,
    /// A newer hold generation (key-up / reschedule) superseded this start
    /// after `session_started` and before the take became an active recording.
    StartSuperseded,
    /// The recorder could not be started after `session_started` was written.
    StartFailed,
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
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub segments: Vec<TranscriptSegment>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub words: Vec<TranscriptWordSpan>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub coverage: Option<TranscriptCoverageReceipt>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pipeline_session_id: Option<String>,
    /// Present only on `session_ended`: why the controller left the session.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub end_reason: Option<TranscriptSessionEndReason>,
    /// Who authored this event. **Absent means the app** — a ledger-observing
    /// [`TranscriptBus`] never sets it, and no app path may. It is set only by
    /// writers that publish text they did not receive from the ledger, today
    /// just [`super::cli_transcript_lane`] with `"cli_file_verdict"`. A reader
    /// that requires occurrence-authenticated truth filters on its absence.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
}

/// Synchronous low-frequency observer. Each lifecycle or authenticated ledger
/// projection is flushed so a live tailer sees it before process exit.
pub struct TranscriptBus {
    session: TranscriptSession,
    path: PathBuf,
    writer: Mutex<TranscriptBusWriter>,
}

/// One lock owns lifecycle and bytes together. This makes the sequence stored
/// on disk authoritative even when engine-close and a late reducer callback
/// arrive from different threads.
struct TranscriptBusWriter {
    file: File,
    sequence: u64,
    started: bool,
    sealed: bool,
    /// The controller left this session; nothing lifecycle-wise follows.
    ended: bool,
    /// Last occurrence-authenticated book projection. `session_ended` may copy
    /// its complete rendered value but can never mutate it.
    last_projection: Option<TranscriptBusEvidenceEvent>,
}

impl TranscriptBus {
    fn projection_availability(
        &self,
        has_text: bool,
        take_in_progress: bool,
        session_wav_exists: bool,
    ) -> TranscriptProjectionAvailability {
        resolve_transcript_projection_availability(
            has_text,
            take_in_progress,
            session_wav_exists,
            self.session.has_latched_target,
            self.session.latched_target_is_self,
        )
    }

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
            vad_open_sample: serial
                .vad_open_sample
                .unwrap_or(serial.occurrence.sample_start),
            vad_close_sample: serial
                .vad_close_sample
                .unwrap_or(serial.occurrence.sample_end),
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
            ReducerAction::RecordLedgerSeal { terminal: true, .. } => "record_ledger_terminal_seal",
            ReducerAction::RecordLedgerSeal {
                terminal: false, ..
            } => "record_ledger_seal",
            ReducerAction::RecordSealCoverage { .. } => "seal_coverage",
            ReducerAction::ApplyManualEdit { .. } => "apply_manual_edit",
            ReducerAction::RecordContextMarker { .. } => "record_context_marker",
        };
        let phase = if reducer_action == "record_ledger_terminal_seal" {
            TranscriptProjectionPhase::Finalizing
        } else {
            TranscriptProjectionPhase::Listening
        };
        let availability =
            self.projection_availability(!revision.rendered_text.trim().is_empty(), true, false);
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
                phase,
                can_paste: availability.can_paste,
                can_insert: availability.can_insert,
                can_copy: availability.can_copy,
                can_retranscribe: availability.can_retranscribe,
                can_format: availability.can_format,
                terminal: false,
                acoustic_receipts: vec![Self::project_serial(
                    serial,
                    entry.word_evidence_receipts.clone(),
                    entry.layer_decision_receipts.clone(),
                    entry.seal_receipt.clone(),
                    entry.manual_edit_receipt.clone(),
                )],
                seal_coverage: revision
                    .seal_coverage
                    .as_ref()
                    .map(ProjectedSealCoverageReceipt::from),
                comparison: revision
                    .comparison
                    .as_ref()
                    .map(ProjectedTranscriptComparisonReceipt::from),
            };
            if let Err(error) = self.write_evidence_event_locked(&mut writer, &event) {
                self.log_write_error(error);
                break;
            }
            writer.last_projection = Some(event.clone());
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
        _sample_rate_override: Option<u32>,
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
                ended: false,
                last_projection: None,
            }),
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

    /// Publish the session's terminal lifecycle line exactly once, and only
    /// after a `session_started` was written. Text-free: it carries no
    /// transcript authority (evidence seals, if any, precede it) — it tells an
    /// observer the controller left this session, so `session_started`
    /// without a later `session_ended` means "take still live", even when
    /// zero occurrences sealed. `reason` is the typed cause; a session that was
    /// superseded or failed before recording says so instead of masquerading
    /// as a completed take.
    pub fn publish_ended(
        &self,
        reason: TranscriptSessionEndReason,
        session_wav_exists: bool,
    ) -> Option<TranscriptBusEvidenceEvent> {
        let mut writer = self
            .writer
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if !writer.started || writer.ended {
            return None;
        }
        let has_text = writer
            .last_projection
            .as_ref()
            .is_some_and(|projection| !projection.rendered_text.trim().is_empty());
        let phase = match reason {
            TranscriptSessionEndReason::Completed if has_text => {
                TranscriptProjectionPhase::Formatted
            }
            TranscriptSessionEndReason::Completed => TranscriptProjectionPhase::NoSpeech,
            TranscriptSessionEndReason::StartSuperseded
            | TranscriptSessionEndReason::StartFailed => TranscriptProjectionPhase::Error,
        };
        let availability = self.projection_availability(has_text, false, session_wav_exists);
        let event = CleanTranscriptEvent {
            schema: "codescribe.transcript.v1".to_string(),
            sequence: 0,
            session_id: String::new(),
            mode: self.session.mode,
            utterance_id: None,
            emitted_at: String::new(),
            status: "session_ended".to_string(),
            sample_rate_hz: None,
            capture_epoch: None,
            sample_start: None,
            sample_end: None,
            audio_start_seconds: None,
            audio_end_seconds: None,
            text: String::new(),
            phase,
            can_paste: availability.can_paste,
            can_insert: availability.can_insert,
            can_copy: availability.can_copy,
            can_retranscribe: availability.can_retranscribe,
            can_format: availability.can_format,
            terminal: true,
            segments: Vec::new(),
            words: Vec::new(),
            coverage: None,
            pipeline_session_id: None,
            end_reason: Some(reason),
            // Ledger-observed: authorship is the app, expressed by absence.
            source: None,
        };
        match self.write_event_locked(&mut writer, event) {
            Ok(()) => {
                writer.ended = true;
                tracing::info!(path = %self.path.display(), session_id = %self.session.session_id, sealed = writer.sealed, ?reason, "clean transcript bus session ended");
                let mut terminal =
                    writer
                        .last_projection
                        .clone()
                        .unwrap_or_else(|| TranscriptBusEvidenceEvent {
                            schema: "codescribe.transcript-evidence.v1".to_string(),
                            sequence: writer.sequence,
                            emitted_at: String::new(),
                            session_id: self.session.session_id.clone(),
                            mode: self.session.mode,
                            reducer_revision: 0,
                            reducer_action: "session_ended".to_string(),
                            occurrence_session_id: String::new(),
                            capture_epoch: 0,
                            sample_start: 0,
                            sample_end: 0,
                            document_index: 0,
                            label: String::new(),
                            rendered_text: String::new(),
                            phase,
                            can_paste: availability.can_paste,
                            can_insert: availability.can_insert,
                            can_copy: availability.can_copy,
                            can_retranscribe: availability.can_retranscribe,
                            can_format: availability.can_format,
                            terminal: true,
                            acoustic_receipts: Vec::new(),
                            seal_coverage: None,
                            comparison: None,
                        });
                terminal.sequence = writer.sequence;
                terminal.emitted_at = Utc::now().to_rfc3339_opts(SecondsFormat::Micros, true);
                terminal.reducer_action = "session_ended".to_string();
                terminal.phase = phase;
                terminal.can_paste = availability.can_paste;
                terminal.can_insert = availability.can_insert;
                terminal.can_copy = availability.can_copy;
                terminal.can_retranscribe = availability.can_retranscribe;
                terminal.can_format = availability.can_format;
                terminal.terminal = true;
                Some(terminal)
            }
            Err(error) => {
                self.log_write_error(error);
                None
            }
        }
    }

    /// The resolved path consumed by an external NDJSON tailer.
    pub fn path(&self) -> &Path {
        &self.path
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
                phase: TranscriptProjectionPhase::Listening,
                can_paste: false,
                can_insert: false,
                can_copy: false,
                can_retranscribe: false,
                can_format: false,
                terminal: false,
                segments: Vec::new(),
                words: Vec::new(),
                coverage: None,
                pipeline_session_id: None,
                end_reason: None,
                // Ledger-observed: authorship is the app, expressed by absence.
                source: None,
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
    fn evidence_v1_without_additive_seal_receipts_still_decodes() {
        let legacy = serde_json::json!({
            "schema": "codescribe.transcript-evidence.v1",
            "sequence": 12,
            "emitted_at": "2026-08-27T22:36:00Z",
            "session_id": "b2b3b95e-4ddc-4845-a5ce-149b21eec166",
            "mode": "dictation",
            "reducer_revision": 7,
            "reducer_action": "record_ledger_terminal_seal",
            "occurrence_session_id": "b2b3b95e-4ddc-4845-a5ce-149b21eec166",
            "capture_epoch": 1,
            "sample_start": 304819,
            "sample_end": 869376,
            "document_index": 0,
            "label": "Kurde",
            "rendered_text": "Kurde",
            "acoustic_receipts": []
        });
        let mut decoded: TranscriptBusEvidenceEvent = serde_json::from_value(legacy).unwrap();
        assert_eq!(decoded.phase, TranscriptProjectionPhase::Listening);
        assert!(!decoded.can_paste);
        assert!(!decoded.can_insert);
        assert!(!decoded.can_copy);
        assert!(!decoded.can_retranscribe);
        assert!(!decoded.can_format);
        assert!(!decoded.terminal);
        assert!(decoded.seal_coverage.is_none());
        assert!(decoded.comparison.is_none());

        decoded.phase = TranscriptProjectionPhase::Formatted;
        decoded.can_paste = true;
        decoded.can_insert = true;
        decoded.can_copy = true;
        decoded.can_retranscribe = true;
        decoded.can_format = true;
        decoded.terminal = true;
        let encoded = serde_json::to_string(&decoded).unwrap();
        let round_trip: TranscriptBusEvidenceEvent = serde_json::from_str(&encoded).unwrap();
        assert_eq!(round_trip, decoded);
    }

    #[test]
    fn bus_flushes_session_lifecycle_privately_without_text_authority() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("events.jsonl");
        let bus = TranscriptBus::open_at(
            TranscriptSession {
                session_id: "session-agent".to_string(),
                mode: TranscriptMode::Agent,
                has_latched_target: false,
                latched_target_is_self: false,
            },
            path.clone(),
            Some(48_000),
        )
        .unwrap();

        bus.publish_started();
        bus.publish_started();

        let lines: Vec<CleanTranscriptEvent> = std::fs::read_to_string(&path)
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0].status, "session_started");
        assert_eq!(lines[0].sequence, 1);
        assert!(lines[0].text.is_empty());

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(path).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
    }

    /// `session_ended` is the text-free terminal lifecycle line: written once,
    /// only after `session_started`, never for a session that never started.
    #[test]
    fn bus_ends_a_started_session_exactly_once_and_never_an_unstarted_one() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("events.jsonl");
        let session = TranscriptSession {
            session_id: "session-ended".to_string(),
            mode: TranscriptMode::Dictation,
            has_latched_target: false,
            latched_target_is_self: false,
        };

        let never_started = TranscriptBus::open_at(session.clone(), path.clone(), None).unwrap();
        let never_started_terminal =
            never_started.publish_ended(TranscriptSessionEndReason::StartSuperseded, false);
        assert!(never_started_terminal.is_none());
        assert_eq!(std::fs::read_to_string(&path).unwrap().lines().count(), 0);

        let bus = TranscriptBus::open_at(session, path.clone(), None).unwrap();
        bus.publish_started();
        let terminal = bus
            .publish_ended(TranscriptSessionEndReason::Completed, false)
            .expect("started session must produce one terminal projection");
        let duplicate_terminal = bus.publish_ended(TranscriptSessionEndReason::StartFailed, false);
        assert!(duplicate_terminal.is_none());
        assert_eq!(terminal.reducer_action, "session_ended");
        assert_eq!(terminal.phase, TranscriptProjectionPhase::NoSpeech);
        assert!(terminal.terminal);
        assert!(terminal.rendered_text.is_empty());

        let lines: Vec<CleanTranscriptEvent> = std::fs::read_to_string(&path)
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0].status, "session_started");
        assert_eq!(lines[1].status, "session_ended");
        assert_eq!(lines[1].sequence, 2);
        assert_eq!(lines[1].session_id, "session-ended");
        assert!(lines[1].text.is_empty());
        assert_eq!(lines[0].end_reason, None);
        // The first terminal wins; a later call with another reason is a no-op.
        assert_eq!(
            lines[1].end_reason,
            Some(TranscriptSessionEndReason::Completed)
        );
    }
}
