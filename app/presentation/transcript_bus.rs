//! Live clean transcript projections with best-effort private NDJSON persistence.
//!
//! The bus observes only occurrence-authenticated revisions emitted by the
//! [`PresentationEmitter`]. It never opens audio, accepts arbitrary text,
//! re-transcribes a file, or reconstructs text from UI deltas.
//!
//! This file keeps the ledger-observing core: the projected acoustic receipts,
//! the evidence event and the revision publication that copies them. The rest
//! of the bus lives in submodules and is re-exported here unchanged:
//! `envelope` (wire schemas, history readers, append descriptors, path),
//! `routing` (channel cohort, publication evidence, closing seal),
//! `attachments` (capture attachments across continuous segments),
//! `playback` (external agent rows, reply playback and delivery controls) and
//! `publication` (session lifecycle, direct revisions, persistence writer).

mod attachments;
mod envelope;
mod playback;
mod publication;
mod routing;
#[cfg(test)]
mod tests;

#[cfg(test)]
#[path = "../../tests/support/p0_b_five_iwo.rs"]
mod p0_b_five_iwo;

use std::collections::HashSet;
use std::io::Write;
use std::path::PathBuf;
use std::sync::Mutex;

use chrono::{SecondsFormat, Utc};
use codescribe_core::pipeline::acoustic_ledger::{
    AcousticLedger, AcousticSerial, IncrementalShapingReceipt, ManualDocumentRevisionReceipt,
    SealCoverageReceipt, TerminalFinalityRefusal,
};
use serde::{Deserialize, Serialize};

use super::emitter::{DirectPublicationCapability, ReducerAction, TranscriptRevision};
use routing::{ChannelPublicationEvidence, CoAudienceSource};

#[cfg(test)]
pub(crate) use envelope::document_history_at;
pub use envelope::{
    CleanTranscriptEvent, DocumentHistoryEntry, ProjectedConsultationMember,
    ProjectedConsultationPresentation, ProjectedPresentationReceipt, ProjectedSealCoverageRange,
    ProjectedTranscriptComparisonReceipt, TRANSCRIPT_BUS_FILENAME, TRANSCRIPT_BUS_PATH_ENV,
    TranscriptCoverageReceipt, TranscriptDelivery, TranscriptMode, TranscriptProjectionPhase,
    TranscriptSession, TranscriptSessionEndReason, TranscriptWordGrain, TranscriptWordSpan,
    document_history, transcript_bus_path,
};
pub(crate) use envelope::{
    HISTORY_SCHEMA, PersistenceDiagnosticSink, compact_history_row, open_bus_append_file,
    shared_bus_file,
};
pub use playback::append_agent_event;
pub(crate) use routing::{ChannelCohort, ChannelCohortMember, ChannelDelivery};

/// Synchronous low-frequency observer. Each lifecycle or authenticated ledger
/// projection attempts a flush; persistence loss cannot suppress live text.
pub struct TranscriptBus {
    session: TranscriptSession,
    path: PathBuf,
    writer: Mutex<TranscriptBusWriter>,
    persistence_diagnostic_sink: PersistenceDiagnosticSink,
}

impl std::fmt::Debug for TranscriptBus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TranscriptBus")
            .field("session", &self.session)
            .field("path", &self.path)
            .finish_non_exhaustive()
    }
}

/// One lock orders live lifecycle and projections. Sequence is in-process
/// publication order, never an acknowledgment of file persistence or delivery.
struct TranscriptBusWriter {
    /// Capture-time recipients are metadata, never transcript identity.
    channel_recipients: Option<serde_json::Value>,
    /// Where this channel reads the other open agent channels from. Metadata
    /// only: never a routing, recipients, identity, CAS or ACK input.
    co_audience: CoAudienceSource,
    /// Disabled for the rest of this session after any uncertain append.
    file: Option<Box<dyn Write + Send>>,
    sequence: u64,
    started: bool,
    sealed: bool,
    /// The controller left this session; nothing lifecycle-wise follows.
    ended: bool,
    end_reason: Option<TranscriptSessionEndReason>,
    capture_epoch: Option<u64>,
    /// Source CAS and refused diagnostics when capture produced no document.
    capture_revision: u64,
    capture_coverage: Option<ProjectedSealCoverageReceipt>,
    capture_comparison: Option<ProjectedTranscriptComparisonReceipt>,
    /// Last occurrence-authenticated book projection. `session_ended` may copy
    /// its complete rendered value but can never mutate it.
    last_projection: Option<TranscriptBusEvidenceEvent>,
    /// The joined controller tail published the final channel wire snapshot.
    /// A reducer seal alone cannot authorize a lifecycle delivery flush.
    channel_seal_published: bool,
    /// Frozen by the joined channel owner, never inferred from display text.
    channel_publication_evidence: Option<ChannelPublicationEvidence>,
    /// Reducer-minted terminal of a ledger-free take: truthful document
    /// completion without an acoustic seal. `sealed` stays ledger-owned; the
    /// channel close reads this marker instead when the take opted out.
    direct_terminal: Option<DirectPublicationCapability>,
    /// Documents whose start row already reached a channel bus. Quiet delivery
    /// contract (Founder seal cc6c8248; only its reopen half was replaced on
    /// 2026-10-26): between that start row and the terminal seal, channel
    /// revisions stay in-process.
    announced_documents: HashSet<u64>,
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
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub word_finality:
        Vec<codescribe_core::pipeline::acoustic_ledger::word_adjudication::WordFinality>,
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
    /// Capture clock carried by the ledger receipt, never inferred by the Bus.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sample_rate_hz: Option<u32>,
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
            sample_rate_hz: receipt.sample_rate_hz,
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
    /// Optional sink-ready bytes, including a selected derived presentation.
    /// `rendered_text` and `reducer_revision` always remain source Raw truth.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub delivery_text: Option<String>,
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
    /// Agent-channel audience. Omitted when the session has none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub audience: Option<String>,
    pub acoustic_receipts: Vec<ProjectedAcousticReceipt>,
    /// Whole-document provenance, independent of word-to-PCM alignment.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub document_revision_receipt: Option<ManualDocumentRevisionReceipt>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seal_coverage: Option<ProjectedSealCoverageReceipt>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub comparison: Option<ProjectedTranscriptComparisonReceipt>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub consultation_presentations: Vec<ProjectedConsultationPresentation>,
    /// A6 uncertain-word spans over `rendered_text`, reducer-computed from
    /// ledger-pinned per-word confidence. Carried in memory to the bridge
    /// projection only: deliberately NOT journaled into the Bus JSONL (d9 —
    /// the Bus stays span-free until thresholds are calibrated), so
    /// `EvidenceRow` consumers never see them.
    #[serde(skip)]
    pub uncertain_spans: Vec<codescribe_core::pipeline::word_confidence::UncertainSpan>,
}

impl TranscriptBus {
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
        word_finality: Vec<
            codescribe_core::pipeline::acoustic_ledger::word_adjudication::WordFinality,
        >,
    ) -> ProjectedAcousticReceipt {
        ProjectedAcousticReceipt {
            word_finality,
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
        // A refused empty capture has no lexical document to publish. Retain
        // only its authenticated source CAS and diagnostics for lifecycle end.
        if revision.authenticates_capture_revision(ledger, &self.session.session_id) {
            self.retain_capture_revision(revision);
            return Vec::new();
        }
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
            ReducerAction::ApplyConsultationPresentation { .. } => {
                "apply_consultation_presentation"
            }
            ReducerAction::RecordContextMarker { .. } => "record_context_marker",
            // The direct lane publishes through publish_direct_revision; a
            // direct revision arriving here fails authentication before this
            // label is ever read.
            ReducerAction::ApplyDirectTranscript { .. } => "apply_direct_transcript",
        };
        let is_user_revision = matches!(&revision.action, ReducerAction::ApplyUserRevision { .. });
        let mut writer = self
            .writer
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let phase = Self::revision_phase(&writer, revision, reducer_action, is_user_revision);
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
        let availability = self.revision_availability(&writer, revision, is_user_revision);
        // Quiet delivery contract (Founder seal cc6c8248, 2026-09-29; only its
        // reopen half was replaced on 2026-10-26): a channel bus carries one
        // start row per document, the terminal seal, manual edits and coverage
        // verdicts. Intermediate revisions stay in-process; they still project
        // to the HUD and in-process consumers below.
        let channel_quiet = self.session.audience.is_some();
        let is_terminal_seal = matches!(
            &revision.action,
            ReducerAction::RecordLedgerSeal { terminal: true, .. }
        );
        let is_coverage_verdict =
            matches!(&revision.action, ReducerAction::RecordSealCoverage { .. });
        let mut emitted = Vec::new();
        let mut persisted = Vec::new();
        if revision.entries.is_empty() {
            return self.publish_document_revision_locked(
                &mut writer,
                revision,
                reducer_action,
                phase,
                &availability,
            );
        }
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
                delivery_text: None,
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
                audience: self.session.audience.clone(),
                acoustic_receipts: vec![Self::project_serial(
                    serial,
                    entry.word_evidence_receipts.clone(),
                    entry.layer_decision_receipts.clone(),
                    entry.seal_receipt.clone(),
                    entry.manual_edit_receipt.clone(),
                    entry.presentation_receipt.as_ref(),
                    ledger
                        .seal_of(&entry.occurrence)
                        .map_or_else(Vec::new, |seal| seal.word_finality.clone()),
                )],
                document_revision_receipt: match &revision.action {
                    ReducerAction::ApplyUserRevision { receipt } => Some(receipt.clone()),
                    _ => None,
                },
                seal_coverage: revision
                    .seal_coverage
                    .as_ref()
                    .map(ProjectedSealCoverageReceipt::from),
                comparison: revision
                    .comparison
                    .as_ref()
                    .map(ProjectedTranscriptComparisonReceipt::from),
                consultation_presentations: revision
                    .consultation_presentations
                    .iter()
                    .map(ProjectedConsultationPresentation::from)
                    .collect(),
                uncertain_spans: revision.uncertain_spans.clone(),
            };
            let starts_document = channel_quiet
                && !writer
                    .announced_documents
                    .contains(&(document_index as u64))
                && !revision.rendered_text.trim().is_empty();
            let persist = !channel_quiet
                || is_terminal_seal
                || is_manual_edit
                || is_coverage_verdict
                || starts_document;
            if persist {
                if starts_document {
                    writer.announced_documents.insert(document_index as u64);
                }
                writer.sequence = event.sequence;
                persisted.push(emitted.len());
            }
            writer.last_projection = Some(event.clone());
            emitted.push(event);
        }
        self.persist_revision_locked(&mut writer, &emitted, &persisted);
        if matches!(
            &revision.action,
            ReducerAction::RecordLedgerSeal { terminal: true, .. }
        ) {
            writer.sealed = true;
        }
        emitted
    }
}
