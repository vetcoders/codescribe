//! Live clean transcript projections with best-effort private NDJSON persistence.
//!
//! The bus observes only occurrence-authenticated revisions emitted by the
//! [`PresentationEmitter`]. It never opens audio, accepts arbitrary text,
//! re-transcribes a file, or reconstructs text from UI deltas.

use std::collections::{HashMap, HashSet};
use std::fs::{File, OpenOptions};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock, Weak};

use chrono::{SecondsFormat, Utc};
use codescribe_core::pipeline::acoustic_ledger::{
    AcousticLedger, AcousticSerial, ConsultationPresentationReceipt, IncrementalShapingReceipt,
    ManualDocumentRevisionReceipt, SealCoverageReceipt, TerminalFinalityRefusal,
    TranscriptComparisonReceipt,
};
use codescribe_core::pipeline::contracts::TranscriptSegment;
use serde::{Deserialize, Serialize};

use super::emitter::{DerivedTranscriptProjection, ReducerAction, TranscriptRevision};
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
    /// Agent-channel audience stamped on sealed rows. `Some("*")` is the Fn+0
    /// broadcast. `None` is every dictation row: readers that ignore the field
    /// keep the previous contract.
    pub audience: Option<String>,
    /// Hold-badge preview only. Paste and the overlay document stay off.
    pub badge_only: bool,
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_seal_receipt: Option<String>,
    #[serde(default)]
    pub sentence_break_before: bool,
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
            sentence_break_before: receipt.sentence_break_before,
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

/// One previously published document revision, read from the Bus journal.
/// This is display evidence, never a mutation input or a substitute reducer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DocumentHistoryEntry {
    pub revision: u64,
    pub rendered_text: String,
    pub provenance: String,
    pub emitted_at: String,
}

/// Durable encoding shares the document once, while live callbacks retain one
/// complete projection per occurrence. The first occurrence remains at the
/// top level for readers that select one document snapshot per revision.
#[derive(Serialize)]
struct DurableRevision<'a> {
    #[serde(flatten)]
    document: &'a TranscriptBusEvidenceEvent,
    persistence_encoding: &'static str,
    occurrence_rows: Vec<DurableOccurrence<'a>>,
}

/// Fields that differ between projections of the same reducer revision.
/// All other fields are copied from the top-level document when expanding.
#[derive(Serialize)]
struct DurableOccurrence<'a> {
    sequence: u64,
    emitted_at: &'a str,
    occurrence_session_id: &'a str,
    capture_epoch: u64,
    sample_start: u64,
    sample_end: u64,
    document_index: u64,
    label: &'a str,
    acoustic_receipts: &'a [ProjectedAcousticReceipt],
}

impl<'a> From<&'a TranscriptBusEvidenceEvent> for DurableOccurrence<'a> {
    fn from(event: &'a TranscriptBusEvidenceEvent) -> Self {
        Self {
            sequence: event.sequence,
            emitted_at: &event.emitted_at,
            occurrence_session_id: &event.occurrence_session_id,
            capture_epoch: event.capture_epoch,
            sample_start: event.sample_start,
            sample_end: event.sample_end,
            document_index: event.document_index,
            label: &event.label,
            acoustic_receipts: &event.acoustic_receipts,
        }
    }
}

pub(crate) const HISTORY_SCHEMA: &str = "codescribe.transcript-history.v1";

#[derive(Serialize, Deserialize)]
struct CompactHistoryRow {
    schema: String,
    session_id: String,
    revision: u64,
    rendered_text: String,
    provenance: String,
    emitted_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    document_revision_receipt: Option<ManualDocumentRevisionReceipt>,
}

/// A compact copy of a reducer revision whose large acoustic row may expire.
/// It retains document provenance; large per-word acoustic rows may expire.
pub(crate) fn compact_history_row(line: &str) -> Option<(String, u64, String)> {
    let event = serde_json::from_str::<TranscriptBusEvidenceEvent>(line).ok()?;
    let entry = history_entry_from_event(&event)?;
    let row = CompactHistoryRow {
        schema: HISTORY_SCHEMA.to_string(),
        session_id: event.session_id.clone(),
        revision: entry.revision,
        rendered_text: entry.rendered_text,
        provenance: entry.provenance,
        emitted_at: entry.emitted_at,
        document_revision_receipt: event.document_revision_receipt.clone(),
    };
    let encoded = serde_json::to_string(&row).ok()?;
    Some((row.session_id, row.revision, encoded))
}

fn history_entry_from_event(event: &TranscriptBusEvidenceEvent) -> Option<DocumentHistoryEntry> {
    if event.document_index != 0
        || event.reducer_revision == 0
        || event.reducer_action == "session_ended"
        || event.rendered_text.trim().is_empty()
    {
        return None;
    }
    let receipt = event
        .document_revision_receipt
        .as_ref()
        .map(|receipt| receipt.receipt_id.as_str())
        .or_else(|| {
            (event.reducer_action == "apply_manual_edit")
                .then(|| {
                    event
                        .acoustic_receipts
                        .first()
                        .and_then(|acoustic| acoustic.manual_edit_receipt.as_deref())
                })
                .flatten()
        });
    let provenance = ["user-edit", "retranscribe", "formatter", "light-plus"]
        .into_iter()
        .find(|kind| receipt.is_some_and(|id| id.starts_with(&format!("{kind}-"))))
        .map(str::to_string)
        .unwrap_or_else(|| match event.reducer_action.as_str() {
            "apply_ledger_decision" => "acoustic-ledger".to_string(),
            "apply_incremental_shaping" => "light-plus".to_string(),
            "apply_consultation_presentation" => "consultation".to_string(),
            action => action.to_string(),
        });
    Some(DocumentHistoryEntry {
        revision: event.reducer_revision,
        rendered_text: event.rendered_text.clone(),
        provenance,
        emitted_at: event.emitted_at.clone(),
    })
}

/// Read the already published history for one take. A missing journal means
/// there is no persisted history; it never licenses reconstructing it in UI.
pub fn document_history(session_id: &str) -> io::Result<Vec<DocumentHistoryEntry>> {
    document_history_at(&transcript_bus_path(), session_id)
}

pub(crate) fn document_history_at(
    path: &Path,
    session_id: &str,
) -> io::Result<Vec<DocumentHistoryEntry>> {
    let mut bytes_read = 0;
    document_history_at_counted(path, session_id, &mut bytes_read)
}

/// Bytes of backward scan one `document_history_at` call will read while
/// looking for this session's `session_started` row. A session that began
/// while persistence was disabled (`TranscriptBus::open_at` refuses to
/// append after a bus whose last byte is not `\n`) never writes that row, so
/// an unbounded scan would walk the whole shared bus — tens of GB on the
/// Founder's machine — for every take. Capped at the same 64 MiB the Swift
/// `OverlayChannelDeliveryReader` tail window and the Rust
/// `agent_ack::FOLD_PASS_BUDGET` / `agent_channel::ORPHAN_SCAN_WINDOW_BYTES`
/// already use, so history degrades to "whatever is in the tail" instead of
/// reading every row ever written.
const DOCUMENT_HISTORY_SCAN_BUDGET_BYTES: u64 = 64 << 20;

/// True if `needle` occurs anywhere in `haystack`. Used as a cheap reject
/// before `serde_json` parsing: a line whose bytes never contain this
/// session's id as a substring cannot have it as the `session_id` field
/// value, so most rows from other interleaved sessions are skipped without
/// building a JSON tree.
fn contains_subslice(haystack: &[u8], needle: &[u8]) -> bool {
    if needle.is_empty() {
        return true;
    }
    if needle.len() > haystack.len() {
        return false;
    }
    haystack
        .windows(needle.len())
        .any(|window| window == needle)
}

fn document_history_at_counted(
    path: &Path,
    session_id: &str,
    bytes_read: &mut u64,
) -> io::Result<Vec<DocumentHistoryEntry>> {
    // nosemgrep: rust.actix.path-traversal.tainted-path.tainted-path -- path is the app-owned transcript_bus_path() or an explicit test temporary Bus path, never request input.
    let mut file = match super::transcript_bus_maintenance::generation::Reader::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error),
    };
    // Recent takes live at the tail. Read blocks backwards and stop at this
    // session's started row; other sessions may be interleaved, so merely
    // seeing another session is not a safe stopping condition.
    let session_bytes = session_id.as_bytes();
    let mut position = file.len();
    let mut prefix = Vec::new();
    let mut rows = Vec::new();
    let mut found_start = false;
    let mut budget_exhausted = false;
    while position > 0 && !found_start {
        let width = position.min(64 * 1024) as usize;
        position -= width as u64;
        file.seek(SeekFrom::Start(position))?;
        let mut block = vec![0; width];
        file.read_exact(&mut block)?;
        *bytes_read += width as u64;
        block.extend_from_slice(&prefix);
        let mut end = block.len();
        for index in (0..block.len()).rev() {
            if block[index] != b'\n' {
                continue;
            }
            let line = &block[index + 1..end];
            end = index;
            if !contains_subslice(line, session_bytes) {
                continue;
            }
            let Ok(row) = serde_json::from_slice::<serde_json::Value>(line) else {
                continue;
            };
            if row
                .get("session_id")
                .or_else(|| row.get("event").and_then(|event| event.get("session_id")))
                .and_then(|value| value.as_str())
                != Some(session_id)
            {
                continue;
            }
            if row.get("status").and_then(|value| value.as_str()) == Some("session_started") {
                found_start = true;
                break;
            }
            rows.push(line.to_vec());
        }
        prefix = block[..end].to_vec();
        if !found_start && *bytes_read >= DOCUMENT_HISTORY_SCAN_BUDGET_BYTES {
            budget_exhausted = true;
            break;
        }
    }
    if !found_start && !prefix.is_empty() {
        rows.push(prefix);
    }
    if budget_exhausted {
        tracing::warn!(
            session_id = %session_id,
            bytes_read = *bytes_read,
            "document_history_at exhausted its scan budget before finding session_started; returning partial tail history"
        );
    }
    rows.reverse();
    let mut storage = super::transcript_bus_maintenance::generation::Decoder::default();
    let mut decoded = Vec::new();
    for line in rows {
        let Ok(value) = serde_json::from_slice(&line) else {
            continue;
        };
        for value in storage.push(value)? {
            // History selects one shared document; validate every occurrence
            // without duplicating its document bytes in this observation.
            let mut rows = super::transcript_bus_maintenance::generation::rows(value)?;
            if let Some(document) = rows.next() {
                decoded.push(serde_json::to_vec(&document).map_err(io::Error::other)?);
            }
        }
    }
    if storage.incomplete() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "incomplete document storage",
        ));
    }
    let mut revisions = std::collections::BTreeMap::new();
    for line in decoded {
        if let Ok(row) = serde_json::from_slice::<CompactHistoryRow>(&line)
            && row.schema == HISTORY_SCHEMA
            && row.session_id == session_id
            && !row.rendered_text.trim().is_empty()
        {
            revisions
                .entry((row.session_id, row.revision))
                .or_insert(DocumentHistoryEntry {
                    revision: row.revision,
                    rendered_text: row.rendered_text,
                    provenance: row.provenance,
                    emitted_at: row.emitted_at,
                });
            continue;
        }
        if let Ok(row) = serde_json::from_slice::<serde_json::Value>(&line)
            && row["schema"] == "codescribe.derived-transcript.v1"
            && row["session_id"] == session_id
            && let (Some(revision), Some(text), Some(mode), Some(time)) = (
                row["revision"].as_u64(),
                row["rendered_text"].as_str(),
                row["requested_mode"].as_str(),
                row["emitted_at"].as_str(),
            )
        {
            revisions
                .entry((session_id.to_string(), revision))
                .or_insert(DocumentHistoryEntry {
                    revision,
                    rendered_text: text.into(),
                    provenance: format!("formatter-{mode}"),
                    emitted_at: time.into(),
                });
            continue;
        }
        let Ok(event) = serde_json::from_slice::<TranscriptBusEvidenceEvent>(&line) else {
            continue;
        };
        if event.session_id != session_id {
            continue;
        }
        if let Some(entry) = history_entry_from_event(&event) {
            revisions
                .entry((event.session_id, entry.revision))
                .or_insert(entry);
        }
    }
    let mut history = revisions.into_values().collect::<Vec<_>>();
    history.sort_by(|left, right| {
        left.emitted_at
            .cmp(&right.emitted_at)
            .then(left.revision.cmp(&right.revision))
    });
    Ok(history)
}

/// Group-level provenance, deliberately separate from per-word acoustic rows.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProjectedConsultationPresentation {
    pub receipt_id: String,
    pub consultation_id: String,
    pub turn_id: String,
    pub source_revision: u64,
    pub revision: u64,
    pub members: Vec<ProjectedConsultationMember>,
    pub rendered_text: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProjectedConsultationMember {
    pub session_id: String,
    pub capture_epoch: u64,
    pub sample_start: u64,
    pub sample_end: u64,
    pub source_label: String,
    pub seal_receipt: String,
}

impl From<&ConsultationPresentationReceipt> for ProjectedConsultationPresentation {
    fn from(receipt: &ConsultationPresentationReceipt) -> Self {
        Self {
            receipt_id: receipt.receipt_id.clone(),
            consultation_id: receipt.consultation_id.clone(),
            turn_id: receipt.turn_id.clone(),
            source_revision: receipt.source_revision,
            revision: receipt.revision,
            rendered_text: receipt.rendered_text.clone(),
            members: receipt
                .members
                .iter()
                .map(|member| ProjectedConsultationMember {
                    session_id: member.occurrence.session.clone(),
                    capture_epoch: member.occurrence.capture_epoch,
                    sample_start: member.occurrence.sample_start,
                    sample_end: member.occurrence.sample_end,
                    source_label: member.source_label.clone(),
                    seal_receipt: member.seal_receipt.clone(),
                })
                .collect(),
        }
    }
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
    /// A synthetic paste accepted the text at the OS boundary.
    SinkAccepted,
    /// The stop path finished without any sink taking the text. It stays
    /// recoverable in the overlay and the session archive.
    Retained,
    /// The text was copied to the clipboard without posting a paste.
    CopiedToClipboard,
    /// The text is armed for a later explicit insert, not pasted yet.
    DeferredInsertArmed,
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

/// Persistence failure reported to the bus's diagnostic sink before it is
/// logged. Text-free by construction: the committed document has no field
/// here, so no observer can ever receive it.
pub(crate) struct PersistenceDiagnostic<'a> {
    pub session_id: &'a str,
    pub file: &'a str,
    pub error: &'a io::Error,
}

/// Injectable observer for persistence failures. Production installs a no-op;
/// tests install a recording sink so assertions never depend on tracing's
/// process-global callsite state.
pub(crate) type PersistenceDiagnosticSink = Arc<dyn Fn(&PersistenceDiagnostic<'_>) + Send + Sync>;

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
    /// Documents whose start row already reached a channel bus. Quiet delivery
    /// contract (Founder seal cc6c8248): between that start row and the
    /// terminal seal, channel revisions stay in-process.
    announced_documents: HashSet<u64>,
}

/// All in-process Bus sessions for a path append through this one descriptor.
/// Compaction takes this exact lock and replaces the descriptor after rename.
static BUS_FILES: OnceLock<Mutex<HashMap<PathBuf, Weak<Mutex<File>>>>> = OnceLock::new();

pub(crate) fn shared_bus_file(path: &Path) -> io::Result<Arc<Mutex<File>>> {
    let registry = BUS_FILES.get_or_init(|| Mutex::new(HashMap::new()));
    let mut registry = registry.lock().unwrap_or_else(|error| error.into_inner());
    if let Some(file) = registry.get(path).and_then(Weak::upgrade) {
        return Ok(file);
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let file = open_bus_append_file(path)?;
    let shared = Arc::new(Mutex::new(file));
    registry.insert(path.to_path_buf(), Arc::downgrade(&shared));
    Ok(shared)
}

pub(crate) fn open_bus_append_file(path: &Path) -> io::Result<File> {
    let mut options = OpenOptions::new();
    options.create(true).append(true).read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let file = options.open(path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        file.set_permissions(std::fs::Permissions::from_mode(0o600))?;
    }
    Ok(file)
}

struct SharedBusWriter(Arc<Mutex<File>>, PathBuf);

/// One session publishes the same encoded row to every destination. An
/// uncertain append retires only that destination, without retrying its prefix.
struct BusFanout {
    session_id: String,
    destinations: Vec<BusDestination>,
}

struct BusDestination {
    path: PathBuf,
    file: Option<Box<dyn Write + Send>>,
}

impl Write for BusFanout {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        for destination in &mut self.destinations {
            let Some(file) = destination.file.as_mut() else {
                continue;
            };
            if let Err(error) = file.write_all(bytes).and_then(|()| file.flush()) {
                destination.file = None;
                tracing::warn!(%error, bus = %destination.path.display(), session_id = %self.session_id, persistence = "disabled_for_session", "transcript bus destination disabled; other destinations continue");
            }
        }
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        // Each destination was flushed as part of the complete-row write.
        Ok(())
    }
}

impl Write for SharedBusWriter {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        super::transcript_bus_maintenance::generation::append(&self.1, &self.0, bytes)?;
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl TranscriptBus {
    /// Copy ownership from the recorder's physical capture-open callback.
    pub(crate) fn observe_capture_opened(&self, session_id: &str, capture_epoch: u64) {
        if self.session.session_id != session_id || capture_epoch == 0 {
            return;
        }
        let mut writer = self
            .writer
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if !writer.ended && writer.capture_epoch.is_none() {
            writer.capture_epoch = Some(capture_epoch);
        }
    }

    /// The actual controller lifecycle must have completed this opened take.
    /// Start failures and superseded starts never authorize a human document.
    pub(crate) fn completed_capture_revision(
        &self,
        session_id: &str,
        capture_epoch: u64,
        source_revision: u64,
    ) -> bool {
        let writer = self
            .writer
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        self.session.session_id == session_id
            && writer.started
            && writer.ended
            && writer.capture_epoch == Some(capture_epoch)
            && matches!(
                writer.end_reason,
                Some(
                    TranscriptSessionEndReason::Completed
                        | TranscriptSessionEndReason::CoverageRefused
                        | TranscriptSessionEndReason::CoverageRefusedEmpty
                        | TranscriptSessionEndReason::DeliveryFailed
                        | TranscriptSessionEndReason::TranscriptionFailed
                )
            )
            && writer
                .last_projection
                .as_ref()
                .map_or(writer.capture_revision, |last| last.reducer_revision)
                == source_revision
    }

    fn projection_availability(
        &self,
        has_text: bool,
        take_in_progress: bool,
        session_wav_exists: bool,
    ) -> TranscriptProjectionAvailability {
        let mut availability = resolve_transcript_projection_availability(
            has_text,
            take_in_progress,
            session_wav_exists,
            self.session.has_latched_target,
            self.session.latched_target_is_self,
        );
        if self.session.badge_only {
            availability.can_paste = false;
            availability.can_insert = false;
            availability.can_send_to_agent = false;
        }
        availability
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
        // A refused empty capture has no lexical document to publish. Retain
        // only its authenticated source CAS and diagnostics for lifecycle end.
        if revision.authenticates_capture_revision(ledger, &self.session.session_id) {
            let mut writer = self
                .writer
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            if !writer.ended && revision.revision > writer.capture_revision {
                writer.capture_revision = revision.revision;
                writer.capture_coverage = revision
                    .seal_coverage
                    .as_ref()
                    .map(ProjectedSealCoverageReceipt::from);
                writer.capture_comparison = revision
                    .comparison
                    .as_ref()
                    .map(ProjectedTranscriptComparisonReceipt::from);
            }
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
        // The authenticated reducer revision carries the ledger's coverage
        // verdict even before session_ended. A document rewrite cannot clear
        // that verdict; the ended row also preserves it for later revisions.
        let phase = if is_user_revision
            && (revision
                .seal_coverage
                .as_ref()
                .is_some_and(|coverage| !coverage.status.is_complete())
                || writer
                    .last_projection
                    .as_ref()
                    .is_some_and(|event| event.phase == TranscriptProjectionPhase::CoverageRefused))
        {
            TranscriptProjectionPhase::CoverageRefused
        } else {
            phase
        };
        let phase = if is_user_revision {
            writer
                .last_projection
                .as_ref()
                .map(|last| last.phase)
                .filter(|phase| {
                    matches!(
                        phase,
                        TranscriptProjectionPhase::Error
                            | TranscriptProjectionPhase::CoverageRefused
                    )
                })
                .unwrap_or(phase)
        } else {
            phase
        };
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
        let availability = if is_user_revision && revision.entries.is_empty() {
            self.projection_availability(
                !revision.rendered_text.trim().is_empty(),
                false,
                writer
                    .last_projection
                    .as_ref()
                    .is_some_and(|last| last.can_retranscribe),
            )
        } else if is_user_revision {
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
        // Quiet delivery contract (Founder seal cc6c8248, 2026-09-29): a channel
        // bus carries one start row per document, the terminal seal, manual
        // edits and coverage verdicts. Intermediate revisions stay in-process;
        // they still project to the HUD and in-process consumers below.
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
            let ReducerAction::ApplyUserRevision { receipt } = &revision.action else {
                return Vec::new();
            };
            if !writer.started
                || !writer.ended
                || writer.capture_epoch != receipt.capture_epoch
                || !matches!(
                    writer.end_reason,
                    Some(
                        TranscriptSessionEndReason::Completed
                            | TranscriptSessionEndReason::CoverageRefused
                            | TranscriptSessionEndReason::CoverageRefusedEmpty
                            | TranscriptSessionEndReason::DeliveryFailed
                            | TranscriptSessionEndReason::TranscriptionFailed
                    )
                )
            {
                return Vec::new();
            }
            let event = TranscriptBusEvidenceEvent {
                schema: "codescribe.transcript-evidence.v1".to_string(),
                sequence: writer.sequence.saturating_add(1),
                emitted_at: Utc::now().to_rfc3339_opts(SecondsFormat::Micros, true),
                session_id: self.session.session_id.clone(),
                mode: self.session.mode,
                reducer_revision: revision.revision,
                reducer_action: reducer_action.to_string(),
                occurrence_session_id: String::new(),
                capture_epoch: receipt.capture_epoch.unwrap_or(0),
                sample_start: 0,
                sample_end: 0,
                document_index: 0,
                label: String::new(),
                rendered_text: revision.rendered_text.clone(),
                delivery_text: None,
                phase,
                can_paste: availability.can_paste,
                can_insert: availability.can_insert,
                can_copy: availability.can_copy,
                can_retranscribe: availability.can_retranscribe,
                can_format: availability.can_format,
                can_send_to_agent: availability.can_send_to_agent,
                terminal: true,
                lifecycle_terminal: false,
                delivery: TranscriptDelivery::Unattempted,
                audience: self.session.audience.clone(),
                acoustic_receipts: Vec::new(),
                document_revision_receipt: Some(receipt.clone()),
                seal_coverage: revision
                    .seal_coverage
                    .as_ref()
                    .map(ProjectedSealCoverageReceipt::from),
                comparison: revision
                    .comparison
                    .as_ref()
                    .map(ProjectedTranscriptComparisonReceipt::from),
                consultation_presentations: Vec::new(),
                uncertain_spans: Vec::new(),
            };
            if let Err(error) = self.write_evidence_event_locked(&mut writer, &event) {
                self.log_write_error(error);
            }
            writer.last_projection = Some(event.clone());
            return vec![event];
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
        if let Some(&first) = persisted.first() {
            let durable = DurableRevision {
                document: &emitted[first],
                persistence_encoding: "shared-revision.v1",
                occurrence_rows: persisted
                    .iter()
                    .skip(1)
                    .map(|&index| DurableOccurrence::from(&emitted[index]))
                    .collect(),
            };
            if let Err(error) = Self::append_projection_locked(&mut writer, &durable) {
                self.log_write_error(error);
            }
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

    /// The production fallback, with an explicit path for local failure
    /// fixtures and for channel sessions routed to a dedicated bus (W5).
    pub(crate) fn open_with_path(session: TranscriptSession, path: PathBuf) -> Self {
        match Self::open_at(session.clone(), path.clone(), None) {
            Ok(bus) => bus,
            Err(error) => {
                let bus = Self::with_writer(session, path, None);
                bus.log_write_error(error);
                bus
            }
        }
    }

    /// Open one lifecycle/sequence with a primary path and additional buses.
    /// Every descriptor comes from the shared registry used by compaction.
    pub(crate) fn open_with_paths(
        session: TranscriptSession,
        path: PathBuf,
        additional_paths: Vec<PathBuf>,
    ) -> Self {
        let bus = Self::open_with_path(session, path.clone());
        if additional_paths.is_empty() {
            return bus;
        }
        let mut writer = bus.writer.lock().unwrap_or_else(|error| error.into_inner());
        let mut destinations = vec![BusDestination {
            path,
            file: writer.file.take(),
        }];
        for path in additional_paths {
            if destinations.iter().any(|known| known.path == path) {
                continue;
            }
            let file = match Self::open_persistence_file(&path) {
                Ok(file) => Some(file),
                Err(error) => {
                    tracing::warn!(%error, bus = %path.display(), session_id = %bus.session.session_id, persistence = "disabled_for_session", "transcript bus destination could not open; other destinations continue");
                    None
                }
            };
            destinations.push(BusDestination { path, file });
        }
        writer.file = Some(Box::new(BusFanout {
            session_id: bus.session.session_id.clone(),
            destinations,
        }));
        drop(writer);
        bus
    }

    /// Channel boundaries share the session's destinations and failure state.
    pub(crate) fn record_channel_receipt(&self, receipt: &serde_json::Value) {
        let mut writer = self
            .writer
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if let Err(error) = Self::append_projection_locked(&mut writer, receipt) {
            self.log_write_error(error);
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
                end_reason: None,
                capture_epoch: None,
                capture_revision: 0,
                capture_coverage: None,
                capture_comparison: None,
                last_projection: None,
                announced_documents: HashSet::new(),
            }),
            persistence_diagnostic_sink: Arc::new(|_| {}),
        }
    }

    /// Test seam: observe persistence failures directly, without a tracing
    /// subscriber. Production logging in `log_write_error` is unchanged.
    #[cfg(test)]
    fn set_persistence_diagnostic_sink(&mut self, sink: PersistenceDiagnosticSink) {
        self.persistence_diagnostic_sink = sink;
    }

    /// Open an explicit path. Kept public for deterministic pipeline tests and
    /// embedders that already own an XDG/project state root.
    pub fn open_at(
        session: TranscriptSession,
        path: PathBuf,
        _sample_rate_override: Option<u32>,
    ) -> io::Result<Self> {
        let file = Self::open_persistence_file(&path)?;
        Ok(Self::with_writer(session, path, Some(file)))
    }

    fn open_persistence_file(path: &Path) -> io::Result<Box<dyn Write + Send>> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        let shared = shared_bus_file(path)?;
        let mut file = shared.lock().unwrap_or_else(|error| error.into_inner());

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
        drop(file);
        Ok(Box::new(SharedBusWriter(shared, path.to_path_buf())))
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

    /// Journal a reducer-minted derived version without advancing the Raw
    /// projection, seal, lifecycle, or delivery disposition. Readers select
    /// this schema explicitly; acoustic document readers continue to see Raw.
    pub(crate) fn record_derived_projection(&self, projection: &DerivedTranscriptProjection) {
        if projection.session_id != self.session.session_id
            || !projection.authenticates_publication()
        {
            return;
        }
        let mut writer = self
            .writer
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if let Err(error) = Self::append_projection_locked(&mut writer, projection) {
            self.log_write_error(error);
        }
    }

    /// UI-only projection of a reducer-authenticated version. It consumes a
    /// sequence but never changes the Bus's retained Raw document or emits a
    /// clean transcript event to an agent observer.
    pub(crate) fn derived_presentation_event(
        &self,
        projection: &DerivedTranscriptProjection,
    ) -> Option<TranscriptBusEvidenceEvent> {
        if !projection.authenticates_publication()
            || projection.session_id != self.session.session_id
        {
            return None;
        }
        let mut writer = self
            .writer
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let mut event = writer.last_projection.clone()?;
        if event.reducer_revision != projection.source_raw_revision
            || event.rendered_text != projection.source_raw_text
        {
            return None;
        }
        writer.sequence = writer.sequence.saturating_add(1);
        event.sequence = writer.sequence;
        event.emitted_at.clone_from(&projection.emitted_at);
        event.reducer_action = "derived_projection".into();
        event.label.clone_from(&projection.receipt_id);
        event.delivery_text = Some(projection.rendered_text.clone());
        event.lifecycle_terminal = false;
        event.delivery = TranscriptDelivery::Unattempted;
        event.uncertain_spans.clear();
        Some(event)
    }

    pub(crate) fn light_plus_deadline_event(
        &self,
        elapsed_ms: u128,
    ) -> Option<TranscriptBusEvidenceEvent> {
        let mut writer = self
            .writer
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let mut event = writer.last_projection.clone()?;
        let receipt = serde_json::json!({
            "schema": "codescribe.light-plus-tick.v1",
            "session_id": self.session.session_id,
            "source_raw_revision": event.reducer_revision,
            "status": "deadline_exceeded",
            "budget_ms": 2000,
            "elapsed_ms": elapsed_ms,
            "delivered": "unchanged_raw",
        });
        if let Err(error) = Self::append_projection_locked(&mut writer, &receipt) {
            self.log_write_error(error);
        }
        writer.sequence = writer.sequence.saturating_add(1);
        event.sequence = writer.sequence;
        event.reducer_action = "light_plus_tick_deadline".into();
        event.label = "Light+ skipped — delivered Raw".into();
        event.lifecycle_terminal = false;
        event.delivery = TranscriptDelivery::Unattempted;
        Some(event)
    }

    pub(crate) fn record_projection_delivery(
        &self,
        projection: &DerivedTranscriptProjection,
        payload: &str,
        disposition: TranscriptDelivery,
    ) {
        if projection.session_id != self.session.session_id
            || !projection.authenticates_publication()
        {
            return;
        }
        let receipt = serde_json::json!({
            "schema": "codescribe.projection-delivery.v1",
            "session_id": projection.session_id,
            "source_raw_revision": projection.source_raw_revision,
            "source_state": projection.source_state,
            "source_raw_text": projection.source_raw_text,
            "projection_receipt": projection.receipt_id,
            "requested_mode": projection.requested_mode,
            "selected_mode": projection.delivered_mode,
            "payload_utf8": payload,
            "disposition": disposition,
        });
        let mut writer = self
            .writer
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if let Err(error) = Self::append_projection_locked(&mut writer, &receipt) {
            self.log_write_error(error);
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
        self.publish_ended_with_delivery_text(reason, session_wav_exists, delivery, None)
    }

    /// End a session while keeping a delivery-only payload distinct from the
    /// reducer-owned document. Only the controller's composer route uses it.
    pub fn publish_ended_with_delivery_text(
        &self,
        reason: TranscriptSessionEndReason,
        session_wav_exists: bool,
        delivery: TranscriptDelivery,
        delivery_text: Option<String>,
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
        writer.end_reason = Some(reason);
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
                    reducer_revision: writer.capture_revision,
                    reducer_action: "session_ended".to_string(),
                    occurrence_session_id: String::new(),
                    capture_epoch: writer.capture_epoch.unwrap_or(0),
                    sample_start: 0,
                    sample_end: 0,
                    document_index: 0,
                    label: String::new(),
                    rendered_text: String::new(),
                    delivery_text: None,
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
                    audience: self.session.audience.clone(),
                    acoustic_receipts: Vec::new(),
                    document_revision_receipt: None,
                    consultation_presentations: Vec::new(),
                    seal_coverage: writer.capture_coverage.clone(),
                    comparison: writer.capture_comparison.clone(),
                    uncertain_spans: Vec::new(),
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
        terminal.delivery_text = delivery_text;
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
        if event.status == "session_ended" {
            // Preserve the reducer's actual source revision even without a
            // prior lexical entry. This metadata does not create a document.
            let mut row = serde_json::to_value(&event).map_err(io::Error::other)?;
            row["reducer_revision"] = serde_json::json!(
                writer
                    .last_projection
                    .as_ref()
                    .map_or(writer.capture_revision, |last| last.reducer_revision)
            );
            row["capture_epoch"] = serde_json::json!(writer.capture_epoch);
            if writer.last_projection.is_none() {
                row["seal_coverage"] =
                    serde_json::to_value(&writer.capture_coverage).map_err(io::Error::other)?;
                row["comparison"] =
                    serde_json::to_value(&writer.capture_comparison).map_err(io::Error::other)?;
            }
            Self::append_projection_locked(writer, &row)
        } else {
            Self::append_projection_locked(writer, &event)
        }
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
            let encoded = super::transcript_bus_maintenance::generation::encode(event)?;
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
        let diagnostic = PersistenceDiagnostic {
            session_id: &self.session.session_id,
            file,
            error: &error,
        };
        (self.persistence_diagnostic_sink)(&diagnostic);
        tracing::warn!(error = %diagnostic.error, file = diagnostic.file, session_id = %diagnostic.session_id, persistence = "disabled_for_session", "clean transcript persistence unavailable; live projection continues without append proof");
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

    /// Serializes tests that emit the persistence-failure `warn!` callsite.
    /// tracing caches each callsite's `Interest` process-globally and
    /// `DefaultCallsite::set_interest` is a plain last-writer-wins store: a
    /// sibling test's first hit of the callsite with no subscriber computes
    /// `Interest::never` from an empty dispatcher registry and can store it
    /// *after* the capturing test rebuilt the cache for its scoped
    /// subscriber, silently disabling the callsite on the capturing thread as
    /// well. Holding this lock for the whole body of every test that can emit
    /// the callsite removes that cross-thread dependency.
    static PERSISTENCE_LOG_SERIAL: Mutex<()> = Mutex::new(());

    fn persistence_log_serial() -> std::sync::MutexGuard<'static, ()> {
        PERSISTENCE_LOG_SERIAL
            .lock()
            .unwrap_or_else(|error| error.into_inner())
    }

    #[test]
    fn history_lists_three_revisions_of_one_take_with_provenance() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("history.jsonl");
        let bus = TranscriptBus::open_at(session("history-take"), path.clone(), None).unwrap();
        bus.publish_started();
        let (ledger, _, base) = committed_fixture("history-take");
        let published = bus.publish_revision(&base, &ledger);
        assert_eq!(published.len(), 2);
        let mut formatter = published[0].clone();
        formatter.reducer_revision += 1;
        formatter.reducer_action = "apply_manual_edit".to_string();
        formatter.rendered_text = "Formatted words".to_string();
        formatter.acoustic_receipts[0].manual_edit_receipt =
            Some("formatter-history-take-2-3-0".to_string());
        let mut retranscribe = formatter.clone();
        retranscribe.reducer_revision += 1;
        retranscribe.rendered_text = "Retranscribed words".to_string();
        retranscribe.acoustic_receipts[0].manual_edit_receipt =
            Some("retranscribe-history-take-3-4-1".to_string());
        let mut file = OpenOptions::new().append(true).open(&path).unwrap();
        writeln!(file, "{}", serde_json::to_string(&formatter).unwrap()).unwrap();
        writeln!(file, "{}", serde_json::to_string(&retranscribe).unwrap()).unwrap();
        let history = document_history_at(&path, "history-take").unwrap();
        assert_eq!(history.len(), 3);
        assert_eq!(history[0].revision, base.revision);
        assert_eq!(history[0].provenance, "acoustic-ledger");
        assert_eq!(history[1].provenance, "formatter");
        assert_eq!(history[2].provenance, "retranscribe");
        assert_eq!(history[2].rendered_text, "Retranscribed words");
        assert!(
            document_history_at(&path, "another-take")
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn recent_take_history_reads_only_the_bus_tail() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("large-history.jsonl");
        let row = format!(
            "{{\"schema\":\"codescribe.transcript.v1\",\"session_id\":\"old\",\"padding\":\"{}\"}}\n",
            "x".repeat(2048)
        );
        let mut out = std::fs::File::create(&path).unwrap();
        while out.metadata().unwrap().len() < 10 * 1024 * 1024 {
            out.write_all(row.as_bytes()).unwrap();
        }
        drop(out);
        let older_bytes = std::fs::metadata(&path).unwrap().len();
        let bus = TranscriptBus::open_at(session("recent-take"), path.clone(), None).unwrap();
        bus.publish_started();
        let (ledger, _, base) = committed_fixture("recent-take");
        assert!(!bus.publish_revision(&base, &ledger).is_empty());
        let recent_bytes =
            crate::durable_bus_oracle::logical_text(&path).len() as u64 - older_bytes;
        let mut bytes_read = 0;
        let history = document_history_at_counted(&path, "recent-take", &mut bytes_read).unwrap();
        assert!(!history.is_empty());
        assert!(
            bytes_read <= recent_bytes + 64 * 1024,
            "read {bytes_read} bytes for {recent_bytes} bytes of recent-session rows"
        );
    }

    /// Regression guard: a bus that still carries this session's
    /// `session_started` row must stop at that row and return exactly the
    /// history it returns today, unaffected by the new scan budget or the
    /// substring prefilter.
    #[test]
    fn history_with_session_started_is_unchanged_by_the_scan_budget() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("history-unchanged.jsonl");
        let bus = TranscriptBus::open_at(session("budget-unaffected"), path.clone(), None).unwrap();
        bus.publish_started();
        let (ledger, _, base) = committed_fixture("budget-unaffected");
        let published = bus.publish_revision(&base, &ledger);
        assert!(!published.is_empty());
        let mut edit = published[0].clone();
        edit.reducer_revision += 1;
        edit.reducer_action = "apply_manual_edit".to_string();
        edit.rendered_text = "Edited words".to_string();
        edit.acoustic_receipts[0].manual_edit_receipt =
            Some("formatter-budget-unaffected-2-3-0".to_string());
        let mut file = OpenOptions::new().append(true).open(&path).unwrap();
        writeln!(file, "{}", serde_json::to_string(&edit).unwrap()).unwrap();
        drop(file);
        let mut bytes_read = 0;
        let history =
            document_history_at_counted(&path, "budget-unaffected", &mut bytes_read).unwrap();
        assert_eq!(history.len(), 2);
        assert_eq!(history[0].revision, base.revision);
        assert_eq!(history[1].provenance, "formatter");
        assert_eq!(history[1].rendered_text, "Edited words");
        assert!(
            bytes_read < DOCUMENT_HISTORY_SCAN_BUDGET_BYTES,
            "a bus carrying session_started must stop well short of the budget"
        );
    }

    /// When a take began while persistence was disabled
    /// (`TranscriptBus::open_at` refuses to append after a bus whose last
    /// byte is not `\n`), `session_started` is never written for it. Before
    /// this cut the backward scan then walked the whole bus; now it must
    /// stop at `DOCUMENT_HISTORY_SCAN_BUDGET_BYTES` and still surface the
    /// tail revision that was actually asked for.
    #[test]
    fn history_without_session_started_stops_at_the_scan_budget_and_keeps_the_tail() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("unbounded.jsonl");
        let filler = format!(
            "{{\"schema\":\"codescribe.transcript.v1\",\"session_id\":\"other-take\",\"padding\":\"{}\"}}\n",
            "x".repeat(4096)
        );
        let mut out = std::fs::File::create(&path).unwrap();
        while out.metadata().unwrap().len() < DOCUMENT_HISTORY_SCAN_BUDGET_BYTES + 8 * 1024 * 1024 {
            out.write_all(filler.as_bytes()).unwrap();
        }
        let padded_len = out.metadata().unwrap().len();

        // Build a real revision event without ever writing `session_started`
        // for it: generate it on a scratch bus, then append the raw line
        // directly, the way an actually-disabled-persistence take would.
        let scratch_path = temp.path().join("scratch.jsonl");
        let scratch_bus =
            TranscriptBus::open_at(session("unbounded-take"), scratch_path, None).unwrap();
        let (ledger, _, base) = committed_fixture("unbounded-take");
        let published = scratch_bus.publish_revision(&base, &ledger);
        assert!(!published.is_empty());
        for event in &published {
            writeln!(out, "{}", serde_json::to_string(event).unwrap()).unwrap();
        }
        drop(out);
        let total_len = std::fs::metadata(&path).unwrap().len();
        assert!(total_len > padded_len);

        let mut bytes_read = 0;
        let history =
            document_history_at_counted(&path, "unbounded-take", &mut bytes_read).unwrap();

        assert!(!history.is_empty(), "tail revision must still surface");
        assert_eq!(history[0].revision, base.revision);
        assert!(
            bytes_read < total_len,
            "scan read {bytes_read} of {total_len} bytes; the budget should stop it short of the whole file"
        );
        assert!(
            bytes_read >= DOCUMENT_HISTORY_SCAN_BUDGET_BYTES,
            "scan stopped at {bytes_read} bytes, short of the {DOCUMENT_HISTORY_SCAN_BUDGET_BYTES} byte budget"
        );
    }

    /// Not run by default: writes and scans a ~1 GiB fixture to measure
    /// `document_history_at`'s wall time with the scan budget and substring
    /// prefilter in place, for a session that never appears in the bus (the
    /// worst case: every row is read but none ever reaches `serde_json`).
    /// Run explicitly with:
    ///   cargo test -p codescribe --lib \
    ///     transcript_bus::tests::document_history_scan_budget_bounds_a_one_gib_bus \
    ///     -- --ignored --nocapture
    #[test]
    #[ignore = "writes and scans a ~1 GiB fixture; run explicitly, see doc comment"]
    fn document_history_scan_budget_bounds_a_one_gib_bus() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("one-gib.jsonl");
        let filler = format!(
            "{{\"schema\":\"codescribe.transcript.v1\",\"session_id\":\"other-take\",\"padding\":\"{}\"}}\n",
            "x".repeat(4096)
        );
        let mut out = std::fs::File::create(&path).unwrap();
        while out.metadata().unwrap().len() < 1024 * 1024 * 1024 {
            out.write_all(filler.as_bytes()).unwrap();
        }
        drop(out);

        let started = std::time::Instant::now();
        let mut bytes_read = 0;
        let history = document_history_at_counted(&path, "absent-take", &mut bytes_read).unwrap();
        let elapsed = started.elapsed();

        assert!(history.is_empty());
        assert!(bytes_read <= DOCUMENT_HISTORY_SCAN_BUDGET_BYTES + 64 * 1024);
        eprintln!("document_history_at scanned {bytes_read} bytes of a 1 GiB bus in {elapsed:?}");
    }

    #[test]
    fn history_versions_match_when_each_revision_carries_text_only_once() {
        let temp = tempfile::tempdir().unwrap();
        let repeated_path = temp.path().join("repeated.jsonl");
        let deduplicated_path = temp.path().join("deduplicated.jsonl");
        let bus =
            TranscriptBus::open_at(session("history-once"), repeated_path.clone(), None).unwrap();
        bus.publish_started();
        let (ledger, _, base) = committed_fixture("history-once");
        let mut versions = vec![bus.publish_revision(&base, &ledger)[0].clone()];
        for offset in 1..=2 {
            let mut next = versions[0].clone();
            next.reducer_revision += offset;
            next.reducer_action = "apply_manual_edit".to_string();
            next.rendered_text = format!("version {}", next.reducer_revision);
            versions.push(next);
        }
        let start = r#"{"schema":"codescribe.transcript.v1","session_id":"history-once","status":"session_started"}"#;
        let mut repeated = format!("{start}\n");
        let mut deduplicated = format!("{start}\n");
        for event in &versions {
            let full = serde_json::to_string(event).unwrap();
            repeated.push_str(&format!("{full}\n{full}\n"));
            let mut without_text = serde_json::to_value(event).unwrap();
            without_text
                .as_object_mut()
                .unwrap()
                .remove("rendered_text");
            deduplicated.push_str(&format!(
                "{full}\n{}\n",
                serde_json::to_string(&without_text).unwrap()
            ));
        }
        std::fs::write(&repeated_path, repeated).unwrap();
        std::fs::write(&deduplicated_path, deduplicated).unwrap();
        let expected = document_history_at(&repeated_path, "history-once").unwrap();
        assert_eq!(expected.len(), 3);
        assert_eq!(
            document_history_at(&deduplicated_path, "history-once").unwrap(),
            expected
        );
    }

    #[test]
    fn managed_journal_refuses_age_rewrite_and_preserves_revision_history() {
        use std::time::{Duration, SystemTime};
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("history-retention.jsonl");
        let bus = TranscriptBus::open_at(session("old-take"), path.clone(), None).unwrap();
        let (ledger, _, base) = committed_fixture("old-take");
        let mut event = bus.publish_revision(&base, &ledger)[0].clone();
        event.emitted_at = "2020-01-01T00:00:00Z".to_string();
        std::fs::write(&path, format!(
            "{{\"schema\":\"codescribe.transcript.v1\",\"session_id\":\"old-take\",\"status\":\"session_started\"}}\n{}\n{{\"schema\":\"codescribe.transcript.v1\",\"session_id\":\"old-take\",\"status\":\"session_ended\"}}\n",
            serde_json::to_string(&event).unwrap()
        )).unwrap();
        let before = document_history_at(&path, "old-take").unwrap();
        assert_eq!(before.len(), 1);
        let file = std::fs::OpenOptions::new().write(true).open(&path).unwrap();
        file.set_times(
            std::fs::FileTimes::new().set_modified(SystemTime::now() - Duration::from_secs(120)),
        )
        .unwrap();
        let original = crate::durable_bus_oracle::logical_text(&path);
        let refused = super::super::transcript_bus_maintenance::compact_bus(&path, 14, false);
        assert!(
            refused.is_err(),
            "managed daily generations never age-drop evidence"
        );
        assert_eq!(crate::durable_bus_oracle::logical_text(&path), original);
        assert_eq!(document_history_at(&path, "old-take").unwrap(), before);
    }

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
            audience: None,
            badge_only: false,
        }
    }

    fn unlatched_dictation(id: &str) -> TranscriptSession {
        TranscriptSession {
            session_id: id.to_string(),
            mode: TranscriptMode::Dictation,
            has_latched_target: false,
            latched_target_is_self: false,
            audience: None,
            badge_only: false,
        }
    }

    #[test]
    fn channel_rows_carry_audience_and_do_not_offer_paste() {
        let dir = tempfile::tempdir().unwrap();
        let mut channel = session("channel-leon");
        channel.audience = Some("Leon".to_string());
        channel.badge_only = true;
        let bus = TranscriptBus::open_at(channel, dir.path().join("channel.jsonl"), None).unwrap();
        let (ledger, _, revision) = committed_fixture("channel-leon");
        let events = bus.publish_revision(&revision, &ledger);
        assert!(!events.is_empty());
        assert!(events.iter().all(|event| {
            event.audience.as_deref() == Some("Leon")
                && !event.can_paste
                && !event.can_insert
                && !event.can_send_to_agent
        }));
        let raw = std::fs::read_to_string(dir.path().join("channel.jsonl")).unwrap();
        assert!(raw.contains("\"audience\":\"Leon\"") || raw.contains("\"audience\": \"Leon\""));

        let plain_dir = tempfile::tempdir().unwrap();
        let plain = TranscriptBus::open_at(
            session("plain-take"),
            plain_dir.path().join("plain.jsonl"),
            None,
        )
        .unwrap();
        let (ledger, _, revision) = committed_fixture("plain-take");
        let events = plain.publish_revision(&revision, &ledger);
        assert!(events.iter().all(|event| event.audience.is_none()));
        let raw = std::fs::read_to_string(plain_dir.path().join("plain.jsonl")).unwrap();
        assert!(
            !raw.contains("\"audience\""),
            "rows without an audience omit the field: {raw}"
        );

        let broadcast_dir = tempfile::tempdir().unwrap();
        let mut broadcast = session("channel-all");
        broadcast.audience = Some("*".to_string());
        broadcast.badge_only = true;
        let bus = TranscriptBus::open_at(broadcast, broadcast_dir.path().join("all.jsonl"), None)
            .unwrap();
        let (ledger, _, revision) = committed_fixture("channel-all");
        let events = bus.publish_revision(&revision, &ledger);
        assert!(
            events
                .iter()
                .all(|event| event.audience.as_deref() == Some("*"))
        );
    }

    /// Integrator (2026-10-01, take agent-channel-0-b351ea82): Fn+0 rows
    /// reached only the shared bus while every follower read its channel bus.
    #[test]
    fn integrator_broadcast_bus_writes_every_row_to_every_destination() {
        let dir = tempfile::tempdir().unwrap();
        let shared = dir.path().join("transcript-events.jsonl");
        let one = dir.path().join("buses/channel-1.jsonl");
        let three = dir.path().join("buses/channel-3.jsonl");
        let mut broadcast = session("agent-channel-0-fanout");
        broadcast.audience = Some("*".to_string());
        broadcast.badge_only = true;
        let bus = TranscriptBus::open_with_paths(
            broadcast,
            shared.clone(),
            vec![one.clone(), three.clone(), one.clone(), shared.clone()],
        );
        bus.publish_started();
        let (ledger, _, revision) = committed_fixture("agent-channel-0-fanout");
        bus.publish_revision(&revision, &ledger);
        bus.record_channel_receipt(&serde_json::json!({
            "schema": "codescribe.channel-session.v1",
            "kind": "channel_session",
            "state": "sealed",
            "channel": "0",
            "session_id": "agent-channel-0-fanout",
        }));
        let read = |path: &Path| std::fs::read_to_string(path).unwrap_or_default();
        let shared_rows = read(&shared);
        assert!(shared_rows.lines().count() >= 2, "{shared_rows}");
        assert_eq!(read(&one), shared_rows, "channel 1 gets the same rows once");
        assert_eq!(
            read(&three),
            shared_rows,
            "channel 3 gets the same rows once"
        );
        assert!(shared_rows.contains("\"audience\":\"*\""), "{shared_rows}");
        assert!(shared_rows.contains("codescribe.channel-session.v1"));
    }

    #[test]
    fn integrator_broken_broadcast_destination_leaves_the_others_writing() {
        let dir = tempfile::tempdir().unwrap();
        let shared = dir.path().join("transcript-events.jsonl");
        let blocker = dir.path().join("not-a-dir");
        std::fs::write(&blocker, b"file").unwrap();
        let broken = blocker.join("channel-2.jsonl");
        let good = dir.path().join("buses/channel-1.jsonl");
        let mut broadcast = session("agent-channel-0-broken");
        broadcast.audience = Some("*".to_string());
        broadcast.badge_only = true;
        let bus = TranscriptBus::open_with_paths(
            broadcast,
            shared.clone(),
            vec![broken.clone(), good.clone()],
        );
        bus.publish_started();
        let (ledger, _, revision) = committed_fixture("agent-channel-0-broken");
        bus.publish_revision(&revision, &ledger);
        let shared_rows = std::fs::read_to_string(&shared).unwrap();
        assert!(!shared_rows.is_empty());
        assert_eq!(std::fs::read_to_string(&good).unwrap(), shared_rows);
        assert!(!broken.exists());
    }

    /// Quiet delivery contract (Founder seal cc6c8248): between one start row
    /// per document and the terminal seal, channel revisions stay off disk.
    #[test]
    fn channel_bus_carries_only_document_starts_and_the_terminal_seal() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("channel.jsonl");
        let mut channel = session("channel-quiet");
        channel.audience = Some("Leon".to_string());
        channel.badge_only = true;
        let bus = TranscriptBus::open_at(channel, path.clone(), None).unwrap();
        bus.publish_started();

        let mut ledger = AcousticLedger::new();
        let mut reducer = TranscriptReducer::default();
        let calibration = EnergyCalibration::new("bus-fault-fixture", 1.0, 1);
        let mut occurrences = Vec::new();
        for (index, label) in ["Zażółć", "gęślą jaźń."].into_iter().enumerate() {
            let start = index as u64 * 16_000;
            let occurrence = OccurrenceIdentity::new("channel-quiet", 7, start, start + 16_000);
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
                occurrence.clone(),
            );
            let receipt = ledger.admit(&observation, label);
            let revision = reducer
                .apply_ledger_mutation(&ledger, &observation, &receipt)
                .expect("live revision");
            // Each revision re-projects every document entry; the second
            // publish therefore re-offers document 0 and must not re-write it.
            let events = bus.publish_revision(&revision, &ledger);
            assert!(
                !events.is_empty(),
                "in-process projections survive quiet persistence"
            );
            occurrences.push(occurrence);
        }
        for occurrence in &occurrences {
            ledger.schedule_frontier(occurrence.clone(), [ObservationProducer::Apple]);
            assert!(ledger.note_frontier_return(occurrence, ObservationProducer::Apple));
        }
        let seal = ledger.seal_terminal("channel-quiet", 7).unwrap();
        let sealed = reducer.apply_ledger_seal(&seal).unwrap();
        assert!(!bus.publish_revision(&sealed, &ledger).is_empty());

        let raw = std::fs::read_to_string(&path).unwrap();
        let rows = crate::durable_bus_oracle::rows(&raw).unwrap();
        let action_counts = rows.iter().fold(
            std::collections::HashMap::<String, usize>::new(),
            |mut counts, row| {
                if let Some(action) = row.get("reducer_action").and_then(|value| value.as_str()) {
                    *counts.entry(action.to_string()).or_default() += 1;
                }
                counts
            },
        );
        assert_eq!(
            action_counts.get("apply_ledger_decision"),
            Some(&2),
            "exactly one start row per document reaches the channel bus: {raw}"
        );
        assert_eq!(
            action_counts.get("record_ledger_terminal_seal"),
            Some(&2),
            "the terminal seal projects once per document entry: {raw}"
        );

        // A dictation session (no audience) keeps every projected row on disk:
        // the same two-admission sequence writes 1 + 2 = 3 decision rows.
        let plain_dir = tempfile::tempdir().unwrap();
        let plain_path = plain_dir.path().join("plain.jsonl");
        let plain =
            TranscriptBus::open_at(unlatched_dictation("plain-loud"), plain_path.clone(), None)
                .unwrap();
        let mut ledger = AcousticLedger::new();
        let mut reducer = TranscriptReducer::default();
        for (index, label) in ["Zażółć", "gęślą jaźń."].into_iter().enumerate() {
            let start = index as u64 * 16_000;
            let occurrence = OccurrenceIdentity::new("plain-loud", 7, start, start + 16_000);
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
            let revision = reducer
                .apply_ledger_mutation(&ledger, &observation, &receipt)
                .expect("live revision");
            assert!(!plain.publish_revision(&revision, &ledger).is_empty());
        }
        let raw = std::fs::read_to_string(&plain_path).unwrap();
        let decisions = crate::durable_bus_oracle::rows(&raw)
            .unwrap()
            .iter()
            .filter(|row| row["reducer_action"] == "apply_ledger_decision")
            .count();
        assert_eq!(decisions, 3, "dictation keeps every revision row: {raw}");
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
    /// Integrator (2026-10-01): the overlay places speech without words only
    /// with the take's own clock; receipts written before the field read as no
    /// clock, never as a guessed rate.
    #[test]
    fn integrator_coverage_projection_carries_the_capture_clock_and_reads_old_rows() {
        use codescribe_core::pipeline::acoustic_ledger::{SealCoverageReceipt, SealCoverageStatus};

        let receipt = SealCoverageReceipt {
            sample_rate_hz: Some(48_000),
            session_id: "clock".into(),
            capture_epoch: 1,
            speech_samples: 96_000,
            covered_samples: 33_600,
            uncovered_speech_ranges: Vec::new(),
            max_uncovered_samples: 62_400,
            incomplete_threshold_samples: 4_000,
            status: SealCoverageStatus::Incomplete,
            speech_producer: "capture_energy".into(),
            availability: "observed".into(),
            observed_samples: Some(96_000),
        };
        let projected = ProjectedSealCoverageReceipt::from(&receipt);
        assert_eq!(projected.sample_rate_hz, Some(48_000));
        let json = serde_json::to_string(&projected).unwrap();
        assert!(json.contains("\"sample_rate_hz\":48000"), "{json}");
        assert_eq!(
            serde_json::from_str::<ProjectedSealCoverageReceipt>(&json).unwrap(),
            projected
        );

        let mut old = projected.clone();
        old.sample_rate_hz = None;
        let old_row = serde_json::to_string(&old).unwrap();
        assert!(!old_row.contains("sample_rate_hz"), "{old_row}");
        assert_eq!(
            serde_json::from_str::<ProjectedSealCoverageReceipt>(&old_row)
                .unwrap()
                .sample_rate_hz,
            None
        );
    }

    #[test]
    fn coverage_projection_round_trips_missing_ratio_and_exact_receipt_equality() {
        use codescribe_core::pipeline::acoustic_ledger::{
            AcousticEvidenceGap, SealCoverageReceipt, SealCoverageStatus,
        };

        let unavailable = SealCoverageReceipt {
            sample_rate_hz: None,
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
                    sample_rate_hz: None,
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
        committed_fixture_with_first_label(id, "Zażółć")
    }

    fn committed_fixture_with_first_label(
        id: &str,
        first_label: &str,
    ) -> (AcousticLedger, TranscriptReducer, TranscriptRevision) {
        let mut ledger = AcousticLedger::new();
        let mut reducer = TranscriptReducer::default();
        let calibration = EnergyCalibration::new("bus-fault-fixture", 1.0, 1);
        let mut revision = None;
        for (index, label) in [first_label, "gęślą\n jaźń."].into_iter().enumerate() {
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
        let _serial = persistence_log_serial();
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
        // End-to-end proof that the production `warn!` still fires with the
        // exact contract content. The serial guard keeps sibling tests from
        // hitting the same callsite while this scoped subscriber's interest
        // is cached (see `PERSISTENCE_LOG_SERIAL`).
        let _serial = persistence_log_serial();
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
            // This test is the only one running that can hit the warn
            // callsite, so recomputing interest with this dispatcher
            // registered settles the cache deterministically.
            tracing::callsite::rebuild_interest_cache();
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
    fn persistence_failure_reaches_the_diagnostic_sink_without_the_document() {
        // The deterministic twin of the tracing capture above: the production
        // failure report is observed through the injected sink, so the
        // assertion cannot depend on tracing's process-global callsite state.
        let _serial = persistence_log_serial();
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("events.jsonl");
        let mut bus = TranscriptBus::open_at(session("sink-fault"), path.clone(), None).unwrap();
        let diagnostics = Arc::new(Mutex::new(Vec::new()));
        let observed = Arc::clone(&diagnostics);
        bus.set_persistence_diagnostic_sink(Arc::new(move |diagnostic| {
            observed.lock().unwrap().push((
                diagnostic.session_id.to_string(),
                diagnostic.file.to_string(),
                diagnostic.error.to_string(),
            ));
        }));
        let fault = inject_fault(&bus);
        fault.lock().unwrap().remaining = Some(0);
        bus.publish_started();
        let (ledger, _, revision) = committed_fixture("sink-fault");
        let events = bus.publish_revision(&revision, &ledger);
        assert_committed(&events, &revision, "sink-fault");
        end_once(&bus, &revision.rendered_text);
        // The first failed append disables persistence for the session, so
        // exactly one diagnostic is reported.
        let diagnostics = diagnostics.lock().unwrap();
        assert_eq!(diagnostics.len(), 1, "diagnostics: {diagnostics:?}");
        let (session_id, file, error) = &diagnostics[0];
        assert_eq!(session_id, "sink-fault");
        assert_eq!(file, "events.jsonl");
        assert!(error.contains("controlled append failure"));
        for payload in [session_id, file, error] {
            assert!(!payload.contains("Zażółć"));
        }
    }

    #[test]
    fn failed_start_and_revision_writes_do_not_suppress_committed_entries() {
        let _serial = persistence_log_serial();
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
        let _serial = persistence_log_serial();
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
                crate::durable_bus_oracle::rows(&crate::durable_bus_oracle::logical_text(&path))
                    .unwrap()
                    .len(),
                if flush { 4 } else { 3 }
            );
        }
    }

    #[test]
    fn prefix_failure_is_not_retried_and_new_sessions_do_not_append_to_it() {
        let _serial = persistence_log_serial();
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

        // A separate explicit journal restores persistence; a stale generation
        // manifest is never silently rewritten to bless a replacement inode.
        let recovered_path = temp.path().join("recovered.jsonl");
        let recovered = TranscriptBus::open_with_path(session("recovered"), recovered_path.clone());
        recovered.publish_started();
        let (ledger, _, revision) = committed_fixture("recovered");
        assert_committed(
            &recovered.publish_revision(&revision, &ledger),
            &revision,
            "recovered",
        );
        end_once(&recovered, &revision.rendered_text);
        assert_eq!(
            std::fs::read(&path).unwrap(),
            partial,
            "failed original journal remains exact"
        );
        let rows = crate::durable_bus_oracle::rows(&crate::durable_bus_oracle::logical_text(
            &recovered_path,
        ))
        .unwrap();
        assert_eq!(rows.len(), 4);
        for row in rows {
            assert_eq!(row["session_id"], "recovered");
        }
    }

    #[test]
    fn uncertain_flush_is_not_replayed_when_sink_recovers() {
        let _serial = persistence_log_serial();
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
        let _serial = persistence_log_serial();
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
                crate::durable_bus_oracle::rows(&crate::durable_bus_oracle::logical_text(&path))
                    .unwrap()
                    .len(),
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
        let bus =
            TranscriptBus::open_at(session("session-agent"), path.clone(), Some(48_000)).unwrap();

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
        let session = unlatched_dictation("session-ended");

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
        let bus = TranscriptBus::open_at(session("session-delivery"), path, None).unwrap();
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
        let bus =
            TranscriptBus::open_at(unlatched_dictation("session-unattempted"), path, None).unwrap();
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
        let (mut ledger, mut reducer, _) =
            committed_fixture_with_first_label("shaping-bytes", "zażółć");
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
        let (mut ledger, mut reducer, _) =
            committed_fixture_with_first_label("shaping-forgery", "zażółć");
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
        let (mut ledger, mut reducer, committed) =
            committed_fixture_with_first_label("shaping-bus", "zażółć");
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
        assert_eq!(committed.rendered_text, "zażółć gęślą\n jaźń.");
        assert_eq!(shaping.rendered_text, "Zażółć gęślą\n jaźń.");
        assert_eq!(events[0].label, "zażółć", "the spoken label is unchanged");

        assert!(
            !bus.writer.lock().unwrap().sealed,
            "an occurrence-level revision must never close the committed book"
        );
    }
    #[test]
    fn retained_shaping_authenticates_the_complete_ordered_snapshot_or_emits_nothing() {
        let (mut ledger, mut reducer, _) =
            committed_fixture_with_first_label("retained-proof", "zażółć");
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
        let (mut without_shaping, _, _) =
            committed_fixture_with_first_label("retained-proof", "zażółć");
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
        let (mut ledger, mut reducer, plain) =
            committed_fixture_with_first_label("serialized-shape", "zażółć");
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

    /// A covered overlap receipt is conserved and visible to the reducer, and
    /// the bus still publishes one committed occurrence.
    #[test]
    fn unanchored_overlap_does_not_publish_a_second_bus_token() {
        use codescribe_core::pipeline::acoustic_ledger::MutationReceipt;

        let temp = tempfile::tempdir().unwrap();
        let bus =
            TranscriptBus::open_with_path(session("unanchored-bus"), temp.path().join("bus.jsonl"));
        bus.publish_started();
        let mut ledger = AcousticLedger::new();
        let mut reducer = TranscriptReducer::default();
        let admitted = OccurrenceIdentity::new("unanchored-bus", 7, 24_000, 48_000);
        let calibration = EnergyCalibration::new("bus-unanchored", 1.0, 1);
        let evidence = AcousticEvidence {
            occurrence: admitted.clone(),
            duration_ms: 1_000.0,
            energy_integral: 10.0,
            mean_rms_dbfs: -12.0,
            peak_dbfs: -3.0,
            vad_open_sample: Some(admitted.sample_start),
            vad_close_sample: Some(admitted.sample_end),
            evidence_calibration_version: calibration.version.clone(),
        };
        assert!(ledger.qualify(&evidence, &calibration).is_qualified());
        let observation =
            ObservationIdentity::new(ObservationProducer::Apple, 1, 0, admitted.clone());
        let receipt = ledger.admit(&observation, "beta");
        let revision = reducer
            .apply_ledger_mutation(&ledger, &observation, &receipt)
            .expect("committed neighbour");
        let events = bus.publish_revision(&revision, &ledger);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].sample_start, 24_000);
        assert_eq!(events[0].sample_end, 48_000);
        assert_eq!(events[0].rendered_text, "beta");

        let covered = OccurrenceIdentity::new("unanchored-bus", 7, 32_000, 40_000);
        let whisper = ObservationIdentity::new(ObservationProducer::Whisper, 2, 0, covered);
        let unanchored = ledger.admit(&whisper, "beta");
        assert!(matches!(
            &unanchored,
            MutationReceipt::KeepVisibleUnanchored { label, .. } if label == "beta"
        ));
        assert!(
            reducer
                .apply_ledger_mutation(&ledger, &whisper, &unanchored)
                .is_none()
        );
        assert_eq!(reducer.visible_projection(), "beta");
        assert_eq!(revision.entries.len(), 1);
        assert_eq!(ledger.conservation().kept_visible_unanchored, 1);
        assert_eq!(ledger.text_of(&admitted), Some("beta"));
        let writer = bus.writer.lock().unwrap();
        assert_eq!(writer.sequence, events[0].sequence);
        assert_eq!(
            writer
                .last_projection
                .as_ref()
                .map(|event| event.rendered_text.as_str()),
            Some("beta")
        );
        assert_eq!(
            writer
                .last_projection
                .as_ref()
                .map(|event| event.sample_end),
            Some(48_000)
        );
    }

    /// A differing alternative kept wholly inside a committed occurrence is
    /// reducer paint evidence only. The next committed revision the Bus
    /// publishes carries the committed label, never the alternative. A seal
    /// does not settle an independent unresolved observation.
    #[test]
    fn differing_unanchored_alternative_never_enters_a_bus_revision() {
        use codescribe_core::pipeline::acoustic_ledger::NoAuthorityReason;

        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("bus.jsonl");
        let bus = TranscriptBus::open_with_path(session("alternative-bus"), path.clone());
        bus.publish_started();
        let mut ledger = AcousticLedger::new();
        let mut reducer = TranscriptReducer::default();
        let apple = OccurrenceIdentity::new("alternative-bus", 7, 0, 48_000);
        let calibration = EnergyCalibration::new("bus-alternative", 1.0, 1);
        let evidence = AcousticEvidence {
            occurrence: apple.clone(),
            duration_ms: 3_000.0,
            energy_integral: 10.0,
            mean_rms_dbfs: -12.0,
            peak_dbfs: -3.0,
            vad_open_sample: Some(apple.sample_start),
            vad_close_sample: Some(apple.sample_end),
            evidence_calibration_version: calibration.version.clone(),
        };
        assert!(ledger.qualify(&evidence, &calibration).is_qualified());
        let observation = ObservationIdentity::new(ObservationProducer::Apple, 1, 0, apple.clone());
        let receipt = ledger.admit(&observation, "Apple mówi tak");
        let revision = reducer
            .apply_ledger_mutation(&ledger, &observation, &receipt)
            .expect("committed Apple occurrence");
        assert_eq!(bus.publish_revision(&revision, &ledger).len(), 1);

        let pin = OccurrenceIdentity::new("alternative-bus", 7, 16_000, 32_000);
        let whisper = ObservationIdentity::new(ObservationProducer::Whisper, 2, 1_000, pin);
        let kept = ledger.keep_visible_unanchored(
            &whisper,
            "Whisper mówi inaczej",
            NoAuthorityReason::ExclusiveTailAwaitingWholeSpan,
        );
        assert!(
            reducer
                .apply_ledger_mutation(&ledger, &whisper, &kept)
                .is_none()
        );
        assert_eq!(reducer.visible_projection(), "Apple mówi tak");
        assert_eq!(
            reducer.unanchored_evidence("alternative-bus", 7)[0].text,
            "Whisper mówi inaczej"
        );

        ledger.schedule_frontier(apple.clone(), [ObservationProducer::Apple]);
        assert!(ledger.note_frontier_return(&apple, ObservationProducer::Apple));
        let seal = ledger.seal(&apple).expect("closed occurrence").clone();
        let sealed = reducer.apply_ledger_seal(&seal).expect("seal revision");
        let events = bus.publish_revision(&sealed, &ledger);
        assert!(!events.is_empty());
        assert!(events.iter().all(|event| {
            !event.rendered_text.contains("Whisper") && !event.label.contains("Whisper")
        }));
        assert_eq!(
            reducer.unanchored_evidence("alternative-bus", 7)[0].text,
            "Whisper mówi inaczej",
            "a containing seal leaves the alternative reviewable outside the Bus"
        );
        assert!(!std::fs::read_to_string(&path).unwrap().contains("Whisper"));
    }

    #[test]
    fn forensic_l77_shared_document_conserves_all_durable_receipts() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("synthetic-bus.jsonl");
        let bus =
            TranscriptBus::open_at(session("synthetic-byte-budget"), path.clone(), None).unwrap();
        bus.publish_started();
        let before = std::fs::metadata(&path).unwrap().len() as usize;
        let label = "x".repeat(4096);
        let (ledger, _, revision) =
            committed_fixture_with_first_label("synthetic-byte-budget", &label);
        let events = bus.publish_revision(&revision, &ledger);
        // In-process HUD and receipt projections retain their existing contract.
        assert_eq!(events.len(), 2);
        assert_committed(&events, &revision, "synthetic-byte-budget");
        let bytes = std::fs::read(&path).unwrap();
        let tail = std::str::from_utf8(&bytes[before..]).unwrap();
        let rows: Vec<serde_json::Value> = tail
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        assert!(!rows.is_empty());
        let projected: Vec<&serde_json::Value> = rows
            .iter()
            .filter(|row| {
                row.get("rendered_text").and_then(|v| v.as_str())
                    == Some(revision.rendered_text.as_str())
            })
            .collect();
        assert!(
            !projected.is_empty(),
            "durable revision must retain the exact full document"
        );
        fn visit_receipts(value: &serde_json::Value, out: &mut Vec<serde_json::Value>) {
            if let serde_json::Value::Object(object) = value {
                if let Some(array) = object.get("acoustic_receipts").and_then(|v| v.as_array()) {
                    out.extend(array.iter().cloned());
                }
                // One full document; every occurrence retains its own receipt.
                if let Some(array) = object.get("occurrence_rows").and_then(|v| v.as_array()) {
                    for row in array {
                        visit_receipts(row, out);
                    }
                }
            }
        }

        let mut persisted_receipts = Vec::new();
        for row in &rows {
            visit_receipts(row, &mut persisted_receipts);
        }
        let expected_receipts: Vec<serde_json::Value> = events
            .iter()
            .flat_map(|event| event.acoustic_receipts.iter())
            .map(|receipt| serde_json::to_value(receipt).unwrap())
            .collect();
        assert_eq!(
            persisted_receipts, expected_receipts,
            "byte reduction must conserve every original occurrence receipt in order"
        );
        let document_bytes: usize = rows
            .iter()
            .filter_map(|row| row.get("rendered_text").and_then(|v| v.as_str()))
            .map(str::len)
            .sum();
        assert!(
            document_bytes <= revision.rendered_text.len() + 64,
            "one durable revision repeated its full document per occurrence: actual {document_bytes}, document {}",
            revision.rendered_text.len()
        );
    }
}
