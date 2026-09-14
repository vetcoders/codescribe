//! Live clean transcript projections with best-effort private NDJSON persistence.
//!
//! The bus observes only occurrence-authenticated revisions emitted by the
//! [`PresentationEmitter`]. It never opens audio, accepts arbitrary text,
//! re-transcribes a file, or reconstructs text from UI deltas.

use std::fs::OpenOptions;
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use chrono::{SecondsFormat, Utc};
use codescribe_core::pipeline::acoustic_ledger::{
    AcousticLedger, AcousticSerial, IncrementalShapingReceipt, SealCoverageReceipt,
    TerminalFinalityRefusal, TranscriptComparisonReceipt,
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
    /// Lifecycle settled with usable words but refused acoustic completeness.
    CoverageRefused,
    NoSpeech,
    Error,
}

impl TranscriptProjectionPhase {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Listening => "listening",
            Self::Finalizing => "finalizing",
            Self::Formatted => "formatted",
            Self::CoverageRefused => "coverage_refused",
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
    /// Absent on plain/legacy entries; absence grants no shaping authority.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub presentation_receipt: Option<ProjectedPresentationReceipt>,
}

/// A complete per-occurrence Light+ proof. Required fields have no defaults.
/// This observer record cannot be submitted to the reducer as mutation authority.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProjectedPresentationReceipt {
    pub receipt_id: String,
    pub provenance: String,
    pub session_id: String,
    pub source_revision: u64,
    pub revision: u64,
    pub capture_epoch: u64,
    pub sample_start: u64,
    pub sample_end: u64,
    pub source_seal_receipt: String,
    pub source_label: String,
    pub left_context: String,
    pub left_context_sha256: String,
    pub shaped_text: String,
}

impl From<&IncrementalShapingReceipt> for ProjectedPresentationReceipt {
    fn from(receipt: &IncrementalShapingReceipt) -> Self {
        Self {
            receipt_id: receipt.receipt_id.clone(),
            provenance: receipt.provenance.clone(),
            session_id: receipt.session_id.clone(),
            source_revision: receipt.source_revision,
            revision: receipt.revision,
            capture_epoch: receipt.occurrence.capture_epoch,
            sample_start: receipt.occurrence.sample_start,
            sample_end: receipt.occurrence.sample_end,
            source_seal_receipt: receipt.source_seal_receipt.clone(),
            source_label: receipt.source_label.clone(),
            left_context: receipt.left_context.clone(),
            left_context_sha256: receipt.left_context_sha256.clone(),
            shaped_text: receipt.shaped_text.clone(),
        }
    }
}

/// One uncovered speech span on the canonical capture PCM clock.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProjectedSealCoverageRange {
    pub sample_start: u64,
    pub sample_end: u64,
}

/// Additive coverage evidence attached to `codescribe.transcript-evidence.v1`.
///
/// # Reader contract
///
/// `status` is one of `complete`, `incomplete`, `unavailable`. A reader
/// branches on that token and, for `unavailable`, on the typed
/// `unavailable_reason` — never on display copy and never on the ratio.
///
/// `coverage_ratio` is **absent** whenever no authenticated acoustic
/// measurement backs the verdict. It is never `NaN` and never a synthetic
/// `1.0`: a take nobody measured has no covered fraction, and rendering one
/// made absence of evidence look like a perfect take. `speech_samples`,
/// `covered_samples` and `max_uncovered_samples` are all `0` in that case and
/// carry no meaning; `observed_samples` is likewise absent.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProjectedSealCoverageReceipt {
    pub status: String,
    /// Typed reason the measurement was missing. Present only when
    /// `status == "unavailable"`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unavailable_reason: Option<String>,
    pub speech_samples: u64,
    pub covered_samples: u64,
    pub uncovered_speech_ranges: Vec<ProjectedSealCoverageRange>,
    pub max_uncovered_samples: u64,
    pub incomplete_threshold_samples: u64,
    /// Which acoustic observer supplied the measurement.
    #[serde(default)]
    pub speech_producer: String,
    /// Availability token reported by that observer.
    #[serde(default)]
    pub availability: String,
    /// Contiguous PCM extent the observer measured; absent when unavailable.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub observed_samples: Option<u64>,
    /// Covered fraction of measured speech; absent when unavailable.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub coverage_ratio: Option<f64>,
}

impl From<&SealCoverageReceipt> for ProjectedSealCoverageReceipt {
    fn from(receipt: &SealCoverageReceipt) -> Self {
        Self {
            status: receipt.status.as_str().to_string(),
            unavailable_reason: receipt
                .status
                .unavailable_reason()
                .map(|gap| gap.as_str().to_string()),
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
            speech_producer: receipt.speech_producer.clone(),
            availability: receipt.availability.clone(),
            observed_samples: receipt.observed_samples,
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
    pub can_send_to_agent: bool,
    #[serde(default)]
    pub terminal: bool,
    /// True only for the session's lifecycle terminal — the line that says the
    /// controller left this session.
    ///
    /// `terminal` alone does not mean that. A committed user/formatter revision
    /// is also terminal: it revises the final document. Conflating the two let a
    /// Light+ or formatter revision release the capture and consume the delivery
    /// slot *before* the lifecycle line carrying the real delivery arrived.
    #[serde(default)]
    pub lifecycle_terminal: bool,
    /// Controller-owned delivery disposition for this take. Only the terminal
    /// lifecycle projection carries anything but [`TranscriptDelivery::Unattempted`];
    /// an evidence revision describes the document, never its destination.
    #[serde(default)]
    pub delivery: TranscriptDelivery,
    pub acoustic_receipts: Vec<ProjectedAcousticReceipt>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seal_coverage: Option<ProjectedSealCoverageReceipt>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub comparison: Option<ProjectedTranscriptComparisonReceipt>,
}

/// Where the stop path sent this take's committed document, as a state an
/// observer branches on. Deliberately typed: the human-facing `label` is
/// presentation and must never carry control meaning, and a route that was
/// *selected* is not a destination that *accepted*.
///
/// The Bus records the controller's disposition only. `ComposerPending` in
/// particular is a standing obligation, not a success: the Agent composer is
/// the intended destination and the receiver has not acknowledged admission.
/// Rust never upgrades it — only the receiver's own typed receipt can.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TranscriptDelivery {
    /// No stop-path delivery ran for this take (start failure, superseded
    /// hold, an abandoned session). Absence of an attempt, not a failure.
    #[default]
    Unattempted,
    /// The document belongs to the Agent composer draft of the thread that
    /// owned the capture. Pending until the receiver admits it.
    ComposerPending,
    /// A system sink (synthetic paste or armed deferred insert) accepted the
    /// text at the OS boundary.
    SinkAccepted,
    /// The stop path finished without any sink taking the text. It stays
    /// recoverable in the overlay and the session archive.
    Retained,
}

/// Why the controller left a Bus session. Typed on purpose: the terminal
/// line carries a reason an observer can branch on, never free text.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TranscriptSessionEndReason {
    /// The take went through the serialized stop path (sealed or zero-seal).
    Completed,
    /// Committed words survived a refused terminal seal; delivery is separate.
    CoverageRefused,
    /// Capture settled with refused coverage and no committed words.
    CoverageRefusedEmpty,
    /// A selected destination failed or declined the committed text.
    DeliveryFailed,
    /// A newer hold generation (key-up / reschedule) superseded this start
    /// after `session_started` and before the take became an active recording.
    StartSuperseded,
    /// The recorder could not be started after `session_started` was written.
    StartFailed,
    /// A CLI file decoder or output sink failed after opening its session.
    TranscriptionFailed,
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
    pub can_send_to_agent: bool,
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
/// projection attempts a flush; persistence loss cannot suppress live text.
pub struct TranscriptBus {
    session: TranscriptSession,
    path: PathBuf,
    writer: Mutex<TranscriptBusWriter>,
}

/// One lock orders live lifecycle and projections. Sequence is in-process
/// publication order, never an acknowledgment of file persistence or delivery.
struct TranscriptBusWriter {
    /// Disabled for the rest of this session after any uncertain append.
    file: Option<Box<dyn Write + Send>>,
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

    pub(crate) fn session_id(&self) -> &str {
        &self.session.session_id
    }

    /// Match a stopped producer's refused document to the already published
    /// reducer receipt. This reads evidence; it cannot publish or repair text.
    pub(crate) fn matches_refused_document(
        &self,
        refusal: &TerminalFinalityRefusal,
        text: &str,
    ) -> bool {
        let writer = self
            .writer
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        writer.started
            && !writer.ended
            && !writer.sealed
            && refusal.session_id() == self.session.session_id
            && writer.last_projection.as_ref().is_some_and(|last| {
                last.capture_epoch == refusal.capture_epoch()
                    && last.rendered_text == text
                    && last.seal_coverage
                        == refusal.coverage().map(ProjectedSealCoverageReceipt::from)
            })
    }

    fn project_serial(
        serial: &AcousticSerial,
        word_evidence_receipts: Vec<String>,
        layer_decision_receipts: Vec<String>,
        seal_receipt: Option<String>,
        manual_edit_receipt: Option<String>,
        presentation_receipt: Option<&IncrementalShapingReceipt>,
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
            presentation_receipt: presentation_receipt.map(ProjectedPresentationReceipt::from),
        }
    }

    /// Observe one reducer revision and copy its ledger receipts byte-for-byte.
    /// The Bus owns only publication sequence and emission time; it cannot admit,
    /// reduce, choose labels, or infer finality.
    pub fn publish_revision(
        &self,
        revision: &TranscriptRevision,
        ledger: &AcousticLedger,
    ) -> Vec<TranscriptBusEvidenceEvent> {
        // Atomic refusal: no partial rows, sequence changes or last-render update.
        // The validator is owned by the reducer, not a second Bus text reducer.
        if !revision.authenticates_publication(ledger, &self.session.session_id) {
            return Vec::new();
        }
        let reducer_action = match &revision.action {
            ReducerAction::ApplyLedgerDecision { .. } => "apply_ledger_decision",
            ReducerAction::RecordLedgerSeal { terminal: true, .. } => "record_ledger_terminal_seal",
            ReducerAction::RecordLedgerSeal {
                terminal: false, ..
            } => "record_ledger_seal",
            ReducerAction::RecordSealCoverage { .. } => "seal_coverage",
            ReducerAction::ApplyManualEdit { .. } | ReducerAction::ApplyUserRevision { .. } => {
                "apply_manual_edit"
            }
            // A live presentation shape of one closed occurrence. It is not a
            // manual edit and not a terminal revision: the words are unchanged,
            // the lifecycle is open, and the take is still being spoken.
            ReducerAction::ApplyIncrementalShaping { .. } => "apply_incremental_shaping",
            ReducerAction::RecordContextMarker { .. } => "record_context_marker",
        };
        let is_user_revision = matches!(&revision.action, ReducerAction::ApplyUserRevision { .. });
        let phase = if is_user_revision {
            TranscriptProjectionPhase::Formatted
        } else if reducer_action == "record_ledger_terminal_seal" {
            TranscriptProjectionPhase::Finalizing
        } else {
            TranscriptProjectionPhase::Listening
        };
        let mut writer = self
            .writer
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if writer
            .last_projection
            .as_ref()
            .is_some_and(|last| revision.revision <= last.reducer_revision)
        {
            return Vec::new();
        }
        let is_manual_edit = matches!(
            &revision.action,
            ReducerAction::ApplyManualEdit { .. } | ReducerAction::ApplyUserRevision { .. }
        );
        if writer.sealed && !is_manual_edit {
            return Vec::new();
        }
        if writer.ended && !is_user_revision {
            return Vec::new();
        }
        let availability = if is_user_revision {
            writer
                .last_projection
                .as_ref()
                .map(|projection| TranscriptProjectionAvailability {
                    can_paste: projection.can_paste,
                    can_insert: projection.can_insert,
                    can_copy: !revision.rendered_text.trim().is_empty(),
                    can_retranscribe: projection.can_retranscribe,
                    can_format: projection.can_format,
                    can_send_to_agent: projection.can_send_to_agent,
                })
                .unwrap_or_else(|| {
                    self.projection_availability(
                        !revision.rendered_text.trim().is_empty(),
                        false,
                        false,
                    )
                })
        } else {
            self.projection_availability(!revision.rendered_text.trim().is_empty(), true, false)
        };
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
                can_send_to_agent: availability.can_send_to_agent,
                terminal: is_user_revision,
                // A revision revises the document; it never ends the session.
                lifecycle_terminal: false,
                // An evidence revision states what the document is, never where
                // it went. Only `publish_ended` stamps a delivery disposition.
                delivery: TranscriptDelivery::Unattempted,
                acoustic_receipts: vec![Self::project_serial(
                    serial,
                    entry.word_evidence_receipts.clone(),
                    entry.layer_decision_receipts.clone(),
                    entry.seal_receipt.clone(),
                    entry.manual_edit_receipt.clone(),
                    entry.presentation_receipt.as_ref(),
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
        Some(Self::open_with_path(session, transcript_bus_path()))
    }

    /// The production fallback, with an explicit path for local failure fixtures.
    fn open_with_path(session: TranscriptSession, path: PathBuf) -> Self {
        match Self::open_at(session.clone(), path.clone(), None) {
            Ok(bus) => bus,
            Err(error) => {
                let bus = Self::with_writer(session, path, None);
                bus.log_write_error(error);
                bus
            }
        }
    }

    fn with_writer(
        session: TranscriptSession,
        path: PathBuf,
        file: Option<Box<dyn Write + Send>>,
    ) -> Self {
        Self {
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
        options.create(true).append(true).read(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(&path)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            file.set_permissions(std::fs::Permissions::from_mode(0o600))?;
        }

        // A prior partial write is not an append boundary. Do not join a new
        // session onto it, truncate evidence, or retry the unknown payload.
        if file.metadata()?.len() > 0 {
            file.seek(SeekFrom::End(-1))?;
            let mut last = [0];
            file.read_exact(&mut last)?;
            if last[0] != b'\n' {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "transcript bus has an incomplete trailing row; persistence disabled",
                ));
            }
        }
        Ok(Self::with_writer(session, path, Some(Box::new(file))))
    }

    /// Announce the recording start exactly once, even if persistence fails.
    /// The controller owns when this happens; retries cannot append another start.
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
    /// after a start was announced in process. Text-free: it carries no
    /// transcript authority (evidence seals, if any, precede it) — it tells an
    /// in-process observer the controller left this session, even when zero
    /// occurrences sealed. A file tailer may miss this line on persistence loss. `reason` is the typed cause; a session that was
    /// superseded or failed before recording says so instead of masquerading
    /// as a completed take.
    ///
    /// `delivery` is the controller's own disposition for the take. The Bus
    /// copies it; it never infers a destination from phase, label or text.
    pub fn publish_ended(
        &self,
        reason: TranscriptSessionEndReason,
        session_wav_exists: bool,
        delivery: TranscriptDelivery,
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
            TranscriptSessionEndReason::CoverageRefused if has_text => {
                TranscriptProjectionPhase::CoverageRefused
            }
            TranscriptSessionEndReason::CoverageRefused
            | TranscriptSessionEndReason::CoverageRefusedEmpty
            | TranscriptSessionEndReason::DeliveryFailed
            | TranscriptSessionEndReason::StartSuperseded
            | TranscriptSessionEndReason::StartFailed
            | TranscriptSessionEndReason::TranscriptionFailed => TranscriptProjectionPhase::Error,
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
            can_send_to_agent: availability.can_send_to_agent,
            terminal: true,
            segments: Vec::new(),
            words: Vec::new(),
            coverage: None,
            pipeline_session_id: None,
            end_reason: Some(reason),
            // Ledger-observed: authorship is the app, expressed by absence.
            source: None,
        };
        if let Err(error) = self.write_event_locked(&mut writer, event) {
            self.log_write_error(error);
        }
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
                    can_send_to_agent: availability.can_send_to_agent,
                    terminal: true,
                    lifecycle_terminal: true,
                    delivery,
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
        terminal.can_send_to_agent = availability.can_send_to_agent;
        terminal.terminal = true;
        // This projection is cloned from the last committed evidence
        // event, which was not a lifecycle line. Say what it now is.
        terminal.lifecycle_terminal = true;
        // The last committed evidence event carried `Unattempted`; the
        // lifecycle line is the one place a disposition is stated.
        terminal.delivery = delivery;
        writer.last_projection = Some(terminal.clone());
        Some(terminal)
    }

    /// The resolved path consumed by an external NDJSON tailer.
    pub fn path(&self) -> &Path {
        &self.path
    }

    fn ensure_started_locked(&self, writer: &mut TranscriptBusWriter) -> io::Result<bool> {
        if writer.started {
            return Ok(false);
        }
        writer.started = true;
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
                can_send_to_agent: false,
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

        writer.sequence = next_sequence;
        Self::append_projection_locked(writer, &event)
    }

    fn write_evidence_event_locked(
        &self,
        writer: &mut TranscriptBusWriter,
        event: &TranscriptBusEvidenceEvent,
    ) -> io::Result<()> {
        writer.sequence = event.sequence;
        Self::append_projection_locked(writer, event)
    }

    fn append_projection_locked(
        writer: &mut TranscriptBusWriter,
        event: &impl Serialize,
    ) -> io::Result<()> {
        let Some(file) = writer.file.as_mut() else {
            return Ok(());
        };
        let result = (|| {
            let mut encoded = serde_json::to_vec(event).map_err(io::Error::other)?;
            encoded.push(b'\n');
            file.write_all(&encoded)?;
            file.flush()
        })();
        if result.is_err() {
            // write_all may already have appended a prefix; flush failure may
            // leave a complete row. Neither permits retry or sequence reuse.
            writer.file = None;
        }
        result
    }

    fn log_write_error(&self, error: io::Error) {
        let file = self
            .path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("transcript-events.jsonl");
        tracing::warn!(%error, file, session_id = %self.session.session_id, persistence = "disabled_for_session", "clean transcript persistence unavailable; live projection continues without append proof");
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
    fn fixture_refusal(receipt: &super::SealCoverageReceipt) -> super::TerminalFinalityRefusal {
        let mut ledger = super::AcousticLedger::new();
        assert!(ledger.record_seal_coverage(receipt.clone()));
        ledger
            .terminal_finality(&receipt.session_id, receipt.capture_epoch)
            .into_refusal()
            .expect("fixture has no issued terminal seal")
    }
    use super::super::emitter::{TranscriptReducer, UserRevisionIntent};
    use super::*;
    use codescribe_core::pipeline::acoustic_ledger::{
        AcousticEvidence, DocumentRevisionProvenance, EnergyCalibration, ObservationIdentity,
        ObservationProducer, OccurrenceIdentity,
    };
    use std::sync::Arc;

    #[derive(Default)]
    struct Fault {
        remaining: Option<usize>,
        flush: bool,
        writes: usize,
        flushes: usize,
    }

    /// Real file-backed writes with controlled prefix/flush failures. Clearing
    /// the fault makes the underlying sink usable, so no-retry is falsifiable.
    struct FaultWriter {
        inner: Box<dyn Write + Send>,
        fault: Arc<Mutex<Fault>>,
    }

    impl Write for FaultWriter {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            let mut fault = self.fault.lock().unwrap();
            fault.writes += 1;
            if fault.remaining == Some(0) {
                return Err(io::Error::other("controlled append failure"));
            }
            let limit = fault.remaining.unwrap_or(bytes.len()).min(bytes.len());
            let written = self.inner.write(&bytes[..limit])?;
            if let Some(remaining) = &mut fault.remaining {
                *remaining -= written;
            }
            Ok(written)
        }

        fn flush(&mut self) -> io::Result<()> {
            let mut fault = self.fault.lock().unwrap();
            fault.flushes += 1;
            if fault.flush {
                return Err(io::Error::other("controlled flush failure"));
            }
            self.inner.flush()
        }
    }

    fn session(id: &str) -> TranscriptSession {
        TranscriptSession {
            session_id: id.to_string(),
            mode: TranscriptMode::Agent,
            has_latched_target: false,
            latched_target_is_self: false,
        }
    }

    fn inject_fault(bus: &TranscriptBus) -> Arc<Mutex<Fault>> {
        let fault = Arc::new(Mutex::new(Fault::default()));
        let mut writer = bus.writer.lock().unwrap();
        let inner = writer.file.take().expect("real file before injection");
        writer.file = Some(Box::new(FaultWriter {
            inner,
            fault: Arc::clone(&fault),
        }));
        fault
    }

    /// UNRUN W2: a lifecycle end preserves the real coverage receipt and never
    /// sets the ledger-seal latch, even when an external sink accepted words.
    #[test]
    fn coverage_refusal_ends_once_without_sealing_the_book() {
        use codescribe_core::audio::capture_receipt::{
            AcousticAvailability, AcousticSpeechEvidence, CaptureEvidenceIdentity,
        };
        use codescribe_core::stt::tail_provider::TailSampleRange;
        let dir = tempfile::tempdir().unwrap();
        let bus =
            TranscriptBus::open_at(session("refused-book"), dir.path().join("bus.jsonl"), None)
                .unwrap();
        bus.publish_started();
        let (mut ledger, mut reducer, _) = committed_fixture("refused-book");
        let receipt = ledger.assess_seal_coverage(
            "refused-book",
            7,
            &AcousticSpeechEvidence::measured(
                CaptureEvidenceIdentity::new("refused-book", 7),
                "capture_energy",
                AcousticAvailability::Observed {
                    observed_samples: 64_000,
                },
                vec![TailSampleRange {
                    session: "refused-book".into(),
                    capture_epoch: 7,
                    sample_start: 0,
                    sample_end: 64_000,
                }],
            ),
            8_000,
        );
        assert!(ledger.record_seal_coverage(receipt.clone()));
        let revision = reducer.apply_seal_coverage(&receipt, None);
        let published = bus.publish_revision(&revision, &ledger);
        assert_eq!(published.len(), 2);
        assert!(bus.matches_refused_document(&fixture_refusal(&receipt), &revision.rendered_text));
        let mut forged = receipt.clone();
        forged.max_uncovered_samples += 1;
        let forged_revision = reducer.apply_seal_coverage(&forged, None);
        assert!(bus.publish_revision(&forged_revision, &ledger).is_empty());
        assert!(bus.matches_refused_document(&fixture_refusal(&receipt), &revision.rendered_text));
        let terminal = bus
            .publish_ended(
                TranscriptSessionEndReason::CoverageRefused,
                true,
                TranscriptDelivery::SinkAccepted,
            )
            .unwrap();
        assert!(!bus.writer.lock().unwrap().sealed);
        assert_eq!(terminal.phase, TranscriptProjectionPhase::CoverageRefused);
        assert_eq!(terminal.delivery, TranscriptDelivery::SinkAccepted);
        assert_eq!(terminal.rendered_text, revision.rendered_text);
        assert_eq!(
            terminal.seal_coverage,
            Some(ProjectedSealCoverageReceipt::from(&receipt))
        );
        assert!(terminal.lifecycle_terminal);
        assert!(terminal.can_retranscribe);
        assert!(
            bus.publish_ended(
                TranscriptSessionEndReason::Completed,
                true,
                TranscriptDelivery::SinkAccepted,
            )
            .is_none()
        );
        assert!(!bus.matches_refused_document(&fixture_refusal(&receipt), &revision.rendered_text));
    }

    /// The projection carries the typed availability reason, an explicitly
    /// absent ratio, and survives a JSON round-trip byte-for-byte — including
    /// the exact-receipt equality the recovery guard depends on.
    #[test]
    fn coverage_projection_round_trips_missing_ratio_and_exact_receipt_equality() {
        use codescribe_core::pipeline::acoustic_ledger::{
            AcousticEvidenceGap, SealCoverageReceipt, SealCoverageStatus,
        };

        let unavailable = SealCoverageReceipt {
            session_id: "round-trip".into(),
            capture_epoch: 7,
            speech_samples: 0,
            covered_samples: 0,
            uncovered_speech_ranges: Vec::new(),
            max_uncovered_samples: 0,
            incomplete_threshold_samples: 4_000,
            status: SealCoverageStatus::Unavailable(AcousticEvidenceGap::NotObserved),
            speech_producer: "capture_energy".into(),
            availability: "not_observed".into(),
            observed_samples: None,
        };
        let projected = ProjectedSealCoverageReceipt::from(&unavailable);
        assert_eq!(projected.status, "unavailable");
        assert_eq!(
            projected.unavailable_reason.as_deref(),
            Some("not_observed")
        );
        assert_eq!(projected.coverage_ratio, None);
        assert_eq!(projected.observed_samples, None);

        let json = serde_json::to_string(&projected).unwrap();
        assert!(
            !json.contains("coverage_ratio"),
            "an absent ratio is absent from the wire, never NaN and never 1.0: {json}"
        );
        assert!(!json.contains("NaN"), "{json}");
        assert!(
            json.contains("\"unavailable_reason\":\"not_observed\""),
            "{json}"
        );
        let decoded: ProjectedSealCoverageReceipt = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded, projected);
        assert_eq!(
            decoded,
            ProjectedSealCoverageReceipt::from(&unavailable),
            "exact projected equality must survive the round-trip"
        );

        // Measured silence keeps a real 1.0 and no reason token.
        let silence = SealCoverageReceipt {
            status: SealCoverageStatus::Complete,
            availability: "observed".into(),
            observed_samples: Some(16_000),
            ..unavailable.clone()
        };
        let projected = ProjectedSealCoverageReceipt::from(&silence);
        assert_eq!(projected.status, "complete");
        assert_eq!(projected.unavailable_reason, None);
        assert_eq!(projected.coverage_ratio, Some(1.0));
        let decoded: ProjectedSealCoverageReceipt =
            serde_json::from_str(&serde_json::to_string(&projected).unwrap()).unwrap();
        assert_eq!(decoded, projected);
        assert_ne!(
            decoded,
            ProjectedSealCoverageReceipt::from(&unavailable),
            "measured silence and absent measurement must not project equal"
        );
    }

    /// A refused document is matched on the whole projected receipt. An
    /// unavailable receipt must match its own projection exactly, and a forged
    /// one — same session, different availability — must not.
    #[test]
    fn refused_document_match_survives_the_added_availability_fields() {
        use codescribe_core::audio::capture_receipt::{
            AcousticAvailability, AcousticSpeechEvidence, CaptureEvidenceIdentity,
        };

        let dir = tempfile::tempdir().unwrap();
        let bus =
            TranscriptBus::open_at(session("refused-avail"), dir.path().join("bus.jsonl"), None)
                .unwrap();
        bus.publish_started();
        let (mut ledger, mut reducer, _) = committed_fixture("refused-avail");
        // No observer measured this take: the words are committed, the extent
        // is unknown, and the ledger refuses on that ground alone.
        let receipt = ledger.assess_seal_coverage(
            "refused-avail",
            7,
            &AcousticSpeechEvidence::unavailable(
                CaptureEvidenceIdentity::new("refused-avail", 7),
                "capture_energy",
                AcousticAvailability::NotObserved,
            ),
            8_000,
        );
        assert!(!receipt.status.is_complete());
        assert_eq!(receipt.coverage_ratio(), None);
        assert!(ledger.record_seal_coverage(receipt.clone()));
        let revision = reducer.apply_seal_coverage(&receipt, None);
        assert_eq!(bus.publish_revision(&revision, &ledger).len(), 2);
        assert!(bus.matches_refused_document(&fixture_refusal(&receipt), &revision.rendered_text));

        let mut forged = receipt.clone();
        forged.availability = "observed".into();
        forged.observed_samples = Some(64_000);
        assert!(
            !bus.matches_refused_document(&fixture_refusal(&forged), &revision.rendered_text),
            "a forged availability must not match the published projection"
        );
        assert!(
            !bus.matches_refused_document(&fixture_refusal(&receipt), "inne słowa"),
            "the document text is still part of the match"
        );
        let mut foreign = receipt.clone();
        foreign.session_id = "successor".into();
        assert!(
            !bus.matches_refused_document(&fixture_refusal(&foreign), &revision.rendered_text),
            "a foreign session never matches this bus"
        );
    }

    /// Synthetic calibrated evidence admitted by the actual ledger and reducer.
    /// Two entries catch an append failure that incorrectly breaks the loop.
    #[test]
    fn finality_refusal_matches_complete_or_absent_coverage_without_inventing_a_seal() {
        for measured in [false, true] {
            let id = if measured {
                "complete-no-seal"
            } else {
                "absent-no-seal"
            };
            let (mut ledger, mut reducer, mut revision) = committed_fixture(id);
            let dir = tempfile::tempdir().unwrap();
            let bus =
                TranscriptBus::open_at(session(id), dir.path().join("bus.jsonl"), None).unwrap();
            bus.publish_started();
            if measured {
                let coverage = SealCoverageReceipt {
                    session_id: id.into(),
                    capture_epoch: 7,
                    speech_samples: 32_000,
                    covered_samples: 32_000,
                    uncovered_speech_ranges: vec![],
                    max_uncovered_samples: 0,
                    incomplete_threshold_samples: 4_000,
                    status:
                        codescribe_core::pipeline::acoustic_ledger::SealCoverageStatus::Complete,
                    speech_producer: "synthetic_test".into(),
                    availability: "observed".into(),
                    observed_samples: Some(32_000),
                };
                assert!(ledger.record_seal_coverage(coverage.clone()));
                revision = reducer.apply_seal_coverage(&coverage, None);
            }
            assert!(!bus.publish_revision(&revision, &ledger).is_empty());
            let refusal = ledger.terminal_finality(id, 7).into_refusal().unwrap();
            assert_eq!(refusal.coverage().is_some(), measured);
            assert!(bus.matches_refused_document(&refusal, &revision.rendered_text));
            assert!(!bus.matches_refused_document(&refusal, "different words"));
            let foreign = ledger.terminal_finality(id, 8).into_refusal().unwrap();
            assert!(!bus.matches_refused_document(&foreign, &revision.rendered_text));
            let missing = AcousticLedger::new()
                .terminal_finality(id, 7)
                .into_refusal()
                .unwrap();
            assert_eq!(
                bus.matches_refused_document(&missing, &revision.rendered_text),
                !measured
            );
            bus.publish_ended(
                TranscriptSessionEndReason::CoverageRefused,
                true,
                TranscriptDelivery::SinkAccepted,
            )
            .unwrap();
            assert!(!bus.matches_refused_document(&refusal, &revision.rendered_text));
            assert!(!bus.writer.lock().unwrap().sealed);
        }
    }

    fn committed_fixture(id: &str) -> (AcousticLedger, TranscriptReducer, TranscriptRevision) {
        let mut ledger = AcousticLedger::new();
        let mut reducer = TranscriptReducer::default();
        let calibration = EnergyCalibration::new("bus-fault-fixture", 1.0, 1);
        let mut revision = None;
        for (index, label) in ["Zażółć", "gęślą\n jaźń."].into_iter().enumerate() {
            let start = index as u64 * 16_000;
            let occurrence = OccurrenceIdentity::new(id, 7, start, start + 16_000);
            let evidence = AcousticEvidence {
                occurrence: occurrence.clone(),
                duration_ms: 1_000.0,
                energy_integral: 10.0,
                mean_rms_dbfs: -12.0,
                peak_dbfs: -3.0,
                vad_open_sample: Some(start),
                vad_close_sample: Some(start + 16_000),
                evidence_calibration_version: calibration.version.clone(),
            };
            assert!(ledger.qualify(&evidence, &calibration).is_qualified());
            let observation = ObservationIdentity::new(
                ObservationProducer::Apple,
                index as u64 + 1,
                0,
                occurrence,
            );
            let receipt = ledger.admit(&observation, label);
            revision = reducer.apply_ledger_mutation(&ledger, &observation, &receipt);
            assert!(revision.is_some());
        }
        (ledger, reducer, revision.unwrap())
    }

    fn assert_committed(
        events: &[TranscriptBusEvidenceEvent],
        revision: &TranscriptRevision,
        id: &str,
    ) {
        assert_eq!(events.len(), 2);
        assert!(!revision.rendered_text.is_empty());
        for (event, entry) in events.iter().zip(&revision.entries) {
            assert_eq!(event.session_id, id);
            assert_eq!(
                event.rendered_text.as_bytes(),
                revision.rendered_text.as_bytes()
            );
            assert_eq!(event.occurrence_session_id, entry.occurrence.session);
            assert_eq!(event.capture_epoch, entry.occurrence.capture_epoch);
            assert_eq!(event.sample_start, entry.occurrence.sample_start);
            assert_eq!(event.sample_end, entry.occurrence.sample_end);
            let receipt = &event.acoustic_receipts[0];
            assert_eq!(receipt.session_id, id);
            assert_eq!(receipt.word_evidence_receipts, entry.word_evidence_receipts);
            assert_eq!(
                receipt.layer_decision_receipts,
                entry.layer_decision_receipts
            );
            assert_eq!(receipt.seal_receipt, entry.seal_receipt);
            assert_eq!(receipt.manual_edit_receipt, entry.manual_edit_receipt);
            assert_eq!(event.delivery, TranscriptDelivery::Unattempted);
            assert!(!event.lifecycle_terminal);
        }
    }

    fn end_once(bus: &TranscriptBus, expected: &str) -> TranscriptBusEvidenceEvent {
        let terminal = bus
            .publish_ended(
                TranscriptSessionEndReason::Completed,
                true,
                TranscriptDelivery::ComposerPending,
            )
            .expect("live terminal survives persistence failure");
        assert_eq!(terminal.rendered_text.as_bytes(), expected.as_bytes());
        assert!(terminal.lifecycle_terminal);
        assert!(terminal.terminal);
        assert!(terminal.can_copy);
        assert_eq!(terminal.delivery, TranscriptDelivery::ComposerPending);
        assert!(
            bus.publish_ended(
                TranscriptSessionEndReason::Completed,
                true,
                TranscriptDelivery::SinkAccepted,
            )
            .is_none()
        );
        assert_eq!(
            bus.writer.lock().unwrap().last_projection.as_ref(),
            Some(&terminal)
        );
        terminal
    }

    #[test]
    fn open_failure_keeps_the_production_bus_and_exact_terminal_text() {
        let temp = tempfile::tempdir().unwrap();
        // Opening a directory as a file fails without touching user permissions.
        let bus = TranscriptBus::open_with_path(session("open-fault"), temp.path().to_path_buf());
        assert!(bus.writer.lock().unwrap().file.is_none());
        bus.publish_started();
        let (ledger, _, revision) = committed_fixture("open-fault");
        let events = bus.publish_revision(&revision, &ledger);
        assert_committed(&events, &revision, "open-fault");
        let terminal = end_once(&bus, &revision.rendered_text);
        assert_eq!(terminal.session_id, "open-fault");
        assert_eq!(terminal.acoustic_receipts, events[1].acoustic_receipts);
        assert_eq!(terminal.sequence, 4);
    }

    #[test]
    fn persistence_failure_is_logged_without_logging_the_committed_document() {
        let temp = tempfile::tempdir().unwrap();
        let log_path = temp.path().join("diagnostics.log");
        let log_file = std::fs::File::create(&log_path).unwrap();
        let subscriber = tracing_subscriber::fmt()
            .without_time()
            .with_ansi(false)
            .with_max_level(tracing::Level::WARN)
            .with_writer(move || log_file.try_clone().unwrap())
            .finish();
        tracing::subscriber::with_default(subscriber, || {
            let bus = TranscriptBus::open_with_path(
                session("diagnostic-fault"),
                temp.path().to_path_buf(),
            );
            bus.publish_started();
            let (ledger, _, revision) = committed_fixture("diagnostic-fault");
            bus.publish_revision(&revision, &ledger);
            end_once(&bus, &revision.rendered_text);
        });
        let log = std::fs::read_to_string(log_path).unwrap();
        assert!(log.contains("diagnostic-fault"));
        assert!(log.contains("disabled_for_session"));
        assert!(log.contains("without append proof"));
        assert!(!log.contains("Zażółć"));
    }

    #[test]
    fn failed_start_and_revision_writes_do_not_suppress_committed_entries() {
        for fail_start in [true, false] {
            let temp = tempfile::tempdir().unwrap();
            let path = temp.path().join("events.jsonl");
            let bus = TranscriptBus::open_at(session("write-fault"), path.clone(), None).unwrap();
            let fault = inject_fault(&bus);
            if fail_start {
                fault.lock().unwrap().remaining = Some(0);
            }
            bus.publish_started();
            bus.publish_started();
            fault.lock().unwrap().remaining = Some(0);
            let (ledger, _, revision) = committed_fixture("write-fault");
            let events = bus.publish_revision(&revision, &ledger);
            assert_committed(&events, &revision, "write-fault");
            assert_eq!(events[0].sequence, 2);
            assert_eq!(events[1].sequence, 3);
            end_once(&bus, &revision.rendered_text);
            assert!(bus.writer.lock().unwrap().file.is_none());
            assert_eq!(
                std::fs::read_to_string(path).unwrap().lines().count(),
                usize::from(!fail_start)
            );
        }
    }

    #[test]
    fn terminal_write_and_flush_failures_keep_one_delivery_obligation() {
        for flush in [false, true] {
            let temp = tempfile::tempdir().unwrap();
            let path = temp.path().join("events.jsonl");
            let bus = TranscriptBus::open_at(session("end-fault"), path.clone(), None).unwrap();
            let fault = inject_fault(&bus);
            bus.publish_started();
            let (ledger, _, revision) = committed_fixture("end-fault");
            let events = bus.publish_revision(&revision, &ledger);
            assert_committed(&events, &revision, "end-fault");
            if flush {
                fault.lock().unwrap().flush = true;
            } else {
                fault.lock().unwrap().remaining = Some(0);
            }
            let terminal = end_once(&bus, &revision.rendered_text);
            assert_eq!(terminal.sequence, 4);
            assert_eq!(terminal.acoustic_receipts, events[1].acoustic_receipts);
            assert!(bus.writer.lock().unwrap().file.is_none());
            assert_eq!(
                std::fs::read_to_string(path).unwrap().lines().count(),
                if flush { 4 } else { 3 }
            );
        }
    }

    #[test]
    fn prefix_failure_is_not_retried_and_new_sessions_do_not_append_to_it() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("events.jsonl");
        let bus = TranscriptBus::open_at(session("prefix"), path.clone(), None).unwrap();
        let fault = inject_fault(&bus);
        bus.publish_started();
        let before = std::fs::read(&path).unwrap();
        fault.lock().unwrap().remaining = Some(19);
        let (ledger, _, revision) = committed_fixture("prefix");
        assert_committed(
            &bus.publish_revision(&revision, &ledger),
            &revision,
            "prefix",
        );
        let partial = std::fs::read(&path).unwrap();
        assert_eq!(partial.len(), before.len() + 19);
        assert!(!partial.ends_with(b"\n"));
        let writes = fault.lock().unwrap().writes;
        fault.lock().unwrap().remaining = None;
        end_once(&bus, &revision.rendered_text);
        assert_eq!(fault.lock().unwrap().writes, writes);
        assert_eq!(std::fs::read(&path).unwrap(), partial);

        let next = TranscriptBus::open_with_path(session("next"), path.clone());
        next.publish_started();
        let empty = next
            .publish_ended(
                TranscriptSessionEndReason::Completed,
                false,
                TranscriptDelivery::Unattempted,
            )
            .unwrap();
        assert!(empty.rendered_text.is_empty());
        assert_eq!(empty.session_id, "next");
        assert_eq!(empty.phase, TranscriptProjectionPhase::NoSpeech);
        assert_eq!(std::fs::read(&path).unwrap(), partial);

        // Explicit fixture rotation restores persistence for a future session;
        // production does not rotate, truncate, or replay unknown bytes itself.
        std::fs::rename(&path, temp.path().join("incomplete.jsonl")).unwrap();
        let recovered = TranscriptBus::open_with_path(session("recovered"), path.clone());
        recovered.publish_started();
        let (ledger, _, revision) = committed_fixture("recovered");
        assert_committed(
            &recovered.publish_revision(&revision, &ledger),
            &revision,
            "recovered",
        );
        end_once(&recovered, &revision.rendered_text);
        let rows = std::fs::read_to_string(path).unwrap();
        assert_eq!(rows.lines().count(), 4);
        for line in rows.lines() {
            let row: serde_json::Value = serde_json::from_str(line).unwrap();
            assert_eq!(row["session_id"], "recovered");
        }
    }

    #[test]
    fn uncertain_flush_is_not_replayed_when_sink_recovers() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("events.jsonl");
        let bus = TranscriptBus::open_at(session("flush"), path.clone(), None).unwrap();
        let fault = inject_fault(&bus);
        bus.publish_started();
        fault.lock().unwrap().flush = true;
        let (ledger, _, revision) = committed_fixture("flush");
        assert_committed(
            &bus.publish_revision(&revision, &ledger),
            &revision,
            "flush",
        );
        let uncertain = std::fs::read(&path).unwrap();
        let writes = fault.lock().unwrap().writes;
        let flushes = fault.lock().unwrap().flushes;
        fault.lock().unwrap().flush = false;
        end_once(&bus, &revision.rendered_text);
        assert_eq!(std::fs::read(&path).unwrap(), uncertain);
        assert_eq!(fault.lock().unwrap().writes, writes);
        assert_eq!(fault.lock().unwrap().flushes, flushes);
        let next = TranscriptBus::open_with_path(session("after-flush"), path.clone());
        next.publish_started();
        let terminal = next
            .publish_ended(
                TranscriptSessionEndReason::Completed,
                false,
                TranscriptDelivery::Unattempted,
            )
            .unwrap();
        assert!(terminal.rendered_text.is_empty());
        let rows = std::fs::read_to_string(path).unwrap();
        assert_eq!(rows.lines().count(), 4);
        for line in rows.lines() {
            let _: serde_json::Value = serde_json::from_str(line).unwrap();
        }
    }

    #[test]
    fn terminal_user_revision_survives_failure_without_reopening_delivery() {
        for failing in [false, true] {
            let temp = tempfile::tempdir().unwrap();
            let path = temp.path().join("events.jsonl");
            let bus = TranscriptBus::open_at(session("edit"), path.clone(), None).unwrap();
            let fault = inject_fault(&bus);
            bus.publish_started();
            let (mut ledger, mut reducer, revision) = committed_fixture("edit");
            assert_committed(&bus.publish_revision(&revision, &ledger), &revision, "edit");
            for occurrence in ledger.occurrences().cloned().collect::<Vec<_>>() {
                ledger.schedule_frontier(occurrence.clone(), [ObservationProducer::Apple]);
                assert!(ledger.note_frontier_return(&occurrence, ObservationProducer::Apple));
            }
            let seal = ledger.seal_terminal("edit", 7).unwrap();
            let sealed = reducer.apply_ledger_seal(&seal).unwrap();
            bus.publish_revision(&sealed, &ledger);
            end_once(&bus, &sealed.rendered_text);
            assert!(bus.publish_revision(&revision, &ledger).is_empty());
            if failing {
                fault.lock().unwrap().remaining = Some(0);
            }
            let edited = reducer
                .apply_user_revision(
                    &mut ledger,
                    &UserRevisionIntent {
                        session_id: "edit".to_string(),
                        source_revision: sealed.revision,
                        rendered_text: "Poprawione — dokładne bajty.\nDrugi wiersz.".to_string(),
                        provenance: DocumentRevisionProvenance::UserEdit,
                    },
                )
                .unwrap();
            let events = bus.publish_revision(&edited, &ledger);
            assert_committed(&events, &edited, "edit");
            assert!(
                events
                    .iter()
                    .all(|event| event.terminal && event.reducer_action == "apply_manual_edit")
            );
            assert!(
                bus.publish_ended(
                    TranscriptSessionEndReason::Completed,
                    true,
                    TranscriptDelivery::ComposerPending,
                )
                .is_none()
            );
            assert_eq!(
                bus.writer.lock().unwrap().last_projection.as_ref(),
                events.last()
            );
            assert_eq!(
                std::fs::read_to_string(path).unwrap().lines().count(),
                if failing { 6 } else { 8 }
            );
        }
    }

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
        let never_started_terminal = never_started.publish_ended(
            TranscriptSessionEndReason::StartSuperseded,
            false,
            TranscriptDelivery::Unattempted,
        );
        assert!(never_started_terminal.is_none());
        assert_eq!(std::fs::read_to_string(&path).unwrap().lines().count(), 0);

        let bus = TranscriptBus::open_at(session, path.clone(), None).unwrap();
        bus.publish_started();
        let terminal = bus
            .publish_ended(
                TranscriptSessionEndReason::Completed,
                false,
                TranscriptDelivery::SinkAccepted,
            )
            .expect("started session must produce one terminal projection");
        let duplicate_terminal = bus.publish_ended(
            TranscriptSessionEndReason::StartFailed,
            false,
            TranscriptDelivery::Unattempted,
        );
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

    /// Acceptance: only the lifecycle line states a destination, and it states
    /// the controller's disposition verbatim.
    ///
    /// The failure this pins is the one that made a delivery contract necessary
    /// at all: an observer that has to infer "where did this go" from a phase or
    /// a display label will eventually infer it wrong.
    #[test]
    fn only_the_lifecycle_line_carries_a_delivery_disposition() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("events.jsonl");
        let bus = TranscriptBus::open_at(
            TranscriptSession {
                session_id: "session-delivery".to_string(),
                mode: TranscriptMode::Agent,
                has_latched_target: false,
                latched_target_is_self: false,
            },
            path,
            None,
        )
        .unwrap();
        bus.publish_started();

        let terminal = bus
            .publish_ended(
                TranscriptSessionEndReason::Completed,
                false,
                TranscriptDelivery::ComposerPending,
            )
            .expect("a started session ends with one terminal projection");

        assert_eq!(terminal.delivery, TranscriptDelivery::ComposerPending);
        assert!(
            terminal.lifecycle_terminal,
            "the line that ends the session must say so"
        );
    }

    /// A take that never delivered says `Unattempted`, which is not the same
    /// claim as "delivered nowhere on purpose" and not the same as a failure.
    #[test]
    fn a_take_with_no_delivery_attempt_claims_no_destination() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("events.jsonl");
        let bus = TranscriptBus::open_at(
            TranscriptSession {
                session_id: "session-unattempted".to_string(),
                mode: TranscriptMode::Dictation,
                has_latched_target: false,
                latched_target_is_self: false,
            },
            path,
            None,
        )
        .unwrap();
        bus.publish_started();

        let terminal = bus
            .publish_ended(
                TranscriptSessionEndReason::StartFailed,
                false,
                TranscriptDelivery::Unattempted,
            )
            .expect("a started session ends with one terminal projection");

        assert_eq!(terminal.delivery, TranscriptDelivery::Unattempted);
    }

    /// A legacy row without the additive fields decodes to the honest defaults:
    /// no destination claimed, and not a lifecycle line.
    #[test]
    fn legacy_rows_default_to_no_destination_and_no_lifecycle_claim() {
        let legacy = serde_json::json!({
            "schema": "codescribe.transcript-evidence.v1",
            "sequence": 1,
            "emitted_at": "2026-09-09T20:00:00Z",
            "session_id": "legacy",
            "mode": "dictation",
            "reducer_revision": 1,
            "reducer_action": "apply_ledger_decision",
            "occurrence_session_id": "legacy",
            "capture_epoch": 1,
            "sample_start": 0,
            "sample_end": 16000,
            "document_index": 0,
            "label": "Kurde",
            "rendered_text": "Kurde",
            "acoustic_receipts": []
        });
        let decoded: TranscriptBusEvidenceEvent = serde_json::from_value(legacy).unwrap();
        assert_eq!(decoded.delivery, TranscriptDelivery::Unattempted);
        assert!(!decoded.lifecycle_terminal);
    }

    // Recovery falsifiers: these contracts are intentionally UNRUN under W2.
    // Preserve predecessor negative controls across the publication recovery.
    #[test]
    fn incremental_bus_refuses_rendered_bytes_not_bound_to_the_shaping_receipt() {
        let (mut ledger, mut reducer, _) = committed_fixture("shaping-bytes");
        let occurrence = OccurrenceIdentity::new("shaping-bytes", 7, 0, 16_000);
        ledger.schedule_frontier(occurrence.clone(), [ObservationProducer::Apple]);
        assert!(ledger.note_frontier_return(&occurrence, ObservationProducer::Apple));
        let seal = ledger.seal(&occurrence).unwrap().clone();
        reducer.apply_ledger_seal(&seal).unwrap();
        let mut revision = reducer
            .apply_incremental_shaping(&mut ledger, &occurrence)
            .unwrap();
        revision.rendered_text = "Unrelated replacement without source authority.".to_string();
        let temp = tempfile::tempdir().unwrap();
        let bus = TranscriptBus::open_at(
            session("shaping-bytes"),
            temp.path().join("bus.jsonl"),
            None,
        )
        .unwrap();
        assert!(bus.publish_revision(&revision, &ledger).is_empty());
        assert!(bus.writer.lock().unwrap().last_projection.is_none());
    }

    #[test]
    fn incremental_bus_refuses_a_shaping_receipt_absent_from_the_ledger() {
        let (mut ledger, mut reducer, _) = committed_fixture("shaping-forgery");
        let occurrence = OccurrenceIdentity::new("shaping-forgery", 7, 0, 16_000);
        ledger.schedule_frontier(occurrence.clone(), [ObservationProducer::Apple]);
        assert!(ledger.note_frontier_return(&occurrence, ObservationProducer::Apple));
        let seal = ledger.seal(&occurrence).unwrap().clone();
        reducer.apply_ledger_seal(&seal).unwrap();
        let mut revision = reducer
            .apply_incremental_shaping(&mut ledger, &occurrence)
            .unwrap();
        let crate::presentation::emitter::ReducerAction::ApplyIncrementalShaping { receipt } =
            &mut revision.action
        else {
            panic!("the real reducer must return a shaping action");
        };
        receipt.receipt_id = "light-plus-incremental-not-minted".to_string();
        let temp = tempfile::tempdir().unwrap();
        let bus = TranscriptBus::open_at(
            session("shaping-forgery"),
            temp.path().join("bus.jsonl"),
            None,
        )
        .unwrap();
        assert!(bus.publish_revision(&revision, &ledger).is_empty());
        assert!(bus.writer.lock().unwrap().last_projection.is_none());
    }

    /// A live per-occurrence shape is observed exactly like any other committed
    /// revision — same evidence rows, same rendered document, same receipts —
    /// but it must not borrow the vocabulary of an edit or of a terminal. The
    /// take is still being spoken, so the book stays open and the projection
    /// stays `listening`.
    #[test]
    fn an_incremental_shaping_publishes_a_listening_revision_without_closing_the_book() {
        let (mut ledger, mut reducer, committed) = committed_fixture("shaping-bus");
        let occurrence = OccurrenceIdentity::new("shaping-bus", 7, 0, 16_000);
        ledger.schedule_frontier(occurrence.clone(), [ObservationProducer::Apple]);
        assert!(ledger.note_frontier_return(&occurrence, ObservationProducer::Apple));
        let seal = ledger
            .seal(&occurrence)
            .expect("a closed qualified occurrence seals")
            .clone();
        assert!(seal.is_occurrence_seal());
        let sealed = reducer
            .apply_ledger_seal(&seal)
            .expect("the occurrence seal projects");

        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("incremental-shaping.jsonl");
        let bus = TranscriptBus::open_at(session("shaping-bus"), path, None).unwrap();
        bus.publish_started();
        assert_eq!(bus.publish_revision(&sealed, &ledger).len(), 2);

        let shaping = reducer
            .apply_incremental_shaping(&mut ledger, &occurrence)
            .expect("a sealed committed occurrence shapes");
        let events = bus.publish_revision(&shaping, &ledger);

        assert_eq!(
            events.len(),
            2,
            "the whole document is projected, not only the shaped span"
        );
        for event in &events {
            assert_eq!(event.reducer_action, "apply_incremental_shaping");
            assert_eq!(event.phase, TranscriptProjectionPhase::Listening);
            assert!(!event.terminal, "a shape is not a terminal revision");
            assert!(!event.lifecycle_terminal, "a shape never ends the session");
            assert_eq!(event.delivery, TranscriptDelivery::Unattempted);
            assert_eq!(event.rendered_text, shaping.rendered_text);
            assert!(
                event.acoustic_receipts[0].manual_edit_receipt.is_none(),
                "a shape must not project as a human correction"
            );
        }
        // Only the shaped occurrence changed; the open one is byte-exact.
        assert_eq!(committed.rendered_text, "Zażółć gęślą\n jaźń.");
        assert_eq!(shaping.rendered_text, "Zażółć. gęślą\n jaźń.");
        assert_eq!(events[0].label, "Zażółć", "the spoken label is unchanged");

        assert!(
            !bus.writer.lock().unwrap().sealed,
            "an occurrence-level revision must never close the committed book"
        );
    }
    #[test]
    fn retained_shaping_authenticates_the_complete_ordered_snapshot_or_emits_nothing() {
        let (mut ledger, mut reducer, _) = committed_fixture("retained-proof");
        let first = OccurrenceIdentity::new("retained-proof", 7, 0, 16_000);
        let second = OccurrenceIdentity::new("retained-proof", 7, 16_000, 32_000);
        for occurrence in [&first, &second] {
            ledger.schedule_frontier(occurrence.clone(), [ObservationProducer::Apple]);
            assert!(ledger.note_frontier_return(occurrence, ObservationProducer::Apple));
            let seal = ledger.seal(occurrence).unwrap().clone();
            reducer.apply_ledger_seal(&seal).unwrap();
        }
        let shaped = reducer
            .apply_incremental_shaping(&mut ledger, &first)
            .unwrap();
        let revision = reducer.record_context_marker(0, "context").unwrap();
        assert!(revision.revision > shaped.revision);
        let temp = tempfile::tempdir().unwrap();
        let bus = TranscriptBus::open_at(session("retained-proof"), temp.path().join("bus"), None)
            .unwrap();
        let mut candidates = Vec::new();
        let mut altered = revision.clone();
        altered.rendered_text.push_str(" injected");
        candidates.push(altered);
        let mut missing = revision.clone();
        missing.entries[0].presentation_receipt = None;
        candidates.push(missing);
        let mut fabricated = revision.clone();
        fabricated.entries[0]
            .presentation_receipt
            .as_mut()
            .unwrap()
            .receipt_id
            .push_str("-forged");
        candidates.push(fabricated);
        let mut stale_source = revision.clone();
        stale_source.entries[0]
            .presentation_receipt
            .as_mut()
            .unwrap()
            .source_revision += 1;
        candidates.push(stale_source);
        let mut reordered = revision.clone();
        reordered.entries.swap(0, 1);
        candidates.push(reordered);
        let mut omitted = revision.clone();
        omitted.entries.pop();
        candidates.push(omitted);
        let mut foreign = revision.clone();
        foreign.entries[1].occurrence.session = "foreign".to_string();
        candidates.push(foreign);
        for candidate in candidates {
            assert!(bus.publish_revision(&candidate, &ledger).is_empty());
            let writer = bus.writer.lock().unwrap();
            assert_eq!(writer.sequence, 0);
            assert!(writer.last_projection.is_none());
        }
        // Same acoustic labels/geometry/seal IDs, but no minted shaping:
        // a valid private snapshot still needs the exact live ledger receipt.
        let (mut without_shaping, _, _) = committed_fixture("retained-proof");
        for occurrence in [&first, &second] {
            without_shaping.schedule_frontier(occurrence.clone(), [ObservationProducer::Apple]);
            assert!(without_shaping.note_frontier_return(occurrence, ObservationProducer::Apple));
            without_shaping.seal(occurrence).unwrap();
        }
        assert!(bus.publish_revision(&revision, &without_shaping).is_empty());
        let rows = bus.publish_revision(&revision, &ledger);
        assert_eq!(rows.len(), 2);
        assert_eq!(
            rows[0].acoustic_receipts[0]
                .presentation_receipt
                .as_ref()
                .unwrap()
                .revision,
            shaped.revision,
            "retained provenance names its original revision"
        );
        assert!(rows[1].acoustic_receipts[0].presentation_receipt.is_none());
        let bytes = std::fs::read(temp.path().join("bus")).unwrap();
        assert!(bus.publish_revision(&revision, &ledger).is_empty());
        assert_eq!(std::fs::read(temp.path().join("bus")).unwrap(), bytes);

        let manual = ObservationIdentity::new(ObservationProducer::ManualHuman, 99, 0, first);
        assert!(ledger.admit(&manual, "replacement").grants_mutation());
        let fresh_bus =
            TranscriptBus::open_at(session("retained-proof"), temp.path().join("stale"), None)
                .unwrap();
        assert!(
            fresh_bus.publish_revision(&revision, &ledger).is_empty(),
            "an intact old snapshot is still refused after its acoustic source changes"
        );
    }

    #[test]
    fn serialized_projection_absence_and_malformed_proof_never_mint_authority() {
        let (mut ledger, mut reducer, plain) = committed_fixture("serialized-shape");
        let temp = tempfile::tempdir().unwrap();
        let bus =
            TranscriptBus::open_at(session("serialized-shape"), temp.path().join("bus"), None)
                .unwrap();
        let plain_rows = bus.publish_revision(&plain, &ledger);
        let old = serde_json::to_value(&plain_rows[0]).unwrap();
        assert!(
            old["acoustic_receipts"][0]
                .get("presentation_receipt")
                .is_none()
        );
        let old: TranscriptBusEvidenceEvent = serde_json::from_value(old).unwrap();
        assert!(old.acoustic_receipts[0].presentation_receipt.is_none());
        let occurrence = OccurrenceIdentity::new("serialized-shape", 7, 0, 16_000);
        ledger.schedule_frontier(occurrence.clone(), [ObservationProducer::Apple]);
        assert!(ledger.note_frontier_return(&occurrence, ObservationProducer::Apple));
        let seal = ledger.seal(&occurrence).unwrap().clone();
        reducer.apply_ledger_seal(&seal).unwrap();
        let revision = reducer
            .apply_incremental_shaping(&mut ledger, &occurrence)
            .unwrap();
        let rows = bus.publish_revision(&revision, &ledger);
        let encoded = serde_json::to_value(&rows[0]).unwrap();
        let decoded: TranscriptBusEvidenceEvent = serde_json::from_value(encoded.clone()).unwrap();
        assert_eq!(
            decoded.acoustic_receipts[0].presentation_receipt,
            rows[0].acoustic_receipts[0].presentation_receipt
        );
        for field in [
            "source_revision",
            "source_seal_receipt",
            "left_context",
            "left_context_sha256",
            "shaped_text",
        ] {
            let mut incomplete = encoded.clone();
            incomplete["acoustic_receipts"][0]["presentation_receipt"]
                .as_object_mut()
                .unwrap()
                .remove(field);
            assert!(serde_json::from_value::<TranscriptBusEvidenceEvent>(incomplete).is_err());
        }
        // A decoded observer record is not a TranscriptRevision capability.
        // Rewriting it never enters publish_revision or creates a ledger receipt.
        let count = ledger.incremental_shapings().len();
        let mut fabricated = encoded;
        fabricated["acoustic_receipts"][0]["presentation_receipt"]["receipt_id"] =
            "not-minted".into();
        let decoded: TranscriptBusEvidenceEvent = serde_json::from_value(fabricated).unwrap();
        assert!(!ledger.incremental_shapings().iter().any(|receipt| {
            receipt.receipt_id
                == decoded.acoustic_receipts[0]
                    .presentation_receipt
                    .as_ref()
                    .unwrap()
                    .receipt_id
        }));
        assert_eq!(ledger.incremental_shapings().len(), count);
    }
}
